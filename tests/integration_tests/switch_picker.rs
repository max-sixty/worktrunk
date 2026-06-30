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

use crate::common::mock_commands::{MockConfig, MockResponse};
use crate::common::{TEST_EPOCH, TestRepo, repo, wt_bin};
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
/// keystroke (Enter to switch, Escape to abort) before force-killing it. A clean
/// switch or abort exits in well under a second, but under heavy CI parallelism
/// the final git work (or skim's Windows terminal teardown) can lag; killing a
/// still-finishing child reports exit 1 on Windows, turning a successful-but-slow
/// switch into a spurious failure. Generous like `READY_TIMEOUT`/`STABILIZE_TIMEOUT`
/// for the same reason — fast polling means the common case still returns at once.
const CHILD_EXIT_TIMEOUT: Duration = Duration::from_secs(30);

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

/// Columns that split the list and preview panels in the 120-col test terminal.
/// skim 4.x draws the │ separator at col 59, with the list to its left (cols
/// 0..59) and the preview interior to its right (cols 60..120). Slicing around
/// col 59 drops the separator from both panels — the border glyph renders
/// inconsistently across platforms.
const LIST_WIDTH: u16 = 59;
const PREVIEW_START_COL: u16 = 60;

/// Result of executing a command in a PTY, holding the parsed terminal state.
struct PtyResult {
    parser: vt100::Parser,
    exit_code: i32,
}

impl PtyResult {
    /// Full screen content as rows of text.
    ///
    /// Trailing whitespace is trimmed from each row because `vt100::rows()` pads
    /// rows to the full column width with spaces. This padding is terminal buffer
    /// fill, not meaningful content, and varies across platforms. Trailing empty
    /// lines are also removed (unwritten terminal rows become empty after trim).
    fn screen(&self) -> String {
        self.parser
            .screen()
            .rows(0, TERM_COLS)
            .map(|row| row.trim_end().to_string())
            .collect::<Vec<_>>()
            .join("\n")
            .trim_end()
            .to_string()
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
    cmd.env("CLICOLOR_FORCE", "1");
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

    let start = Instant::now();
    // Generous like the keystroke-driven helpers: a slow-but-successful exit that
    // gets killed reports exit 1 on Windows — see `CHILD_EXIT_TIMEOUT`.
    let timeout = CHILD_EXIT_TIMEOUT;
    loop {
        while rx.try_recv().is_ok() {} // discard chunks
        if child.try_wait().unwrap().is_some() {
            break;
        }
        if start.elapsed() >= timeout {
            let _ = child.kill();
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }

    child.wait().unwrap().exit_code() as i32
}

/// Execute a command in a PTY and return the parsed terminal state.
///
/// Uses polling with stabilization detection instead of fixed delays.
fn exec_in_pty_with_input(
    command: &str,
    args: &[&str],
    working_dir: &Path,
    env_vars: &[(String, String)],
    input: &str,
) -> PtyResult {
    exec_in_pty_with_input_expectations(command, args, working_dir, env_vars, &[(input, None)])
}

/// Execute a command in a PTY with a sequence of inputs and optional content expectations.
///
/// Each input is `(input_bytes, expected_content)`:
/// - `expected_content`: a substring that must appear on screen before the input is considered
///   processed. Required for async preview content that lands later than the prompt update.
///
/// Example: `[("\x1b[B", None), ("\x1b3", Some("diff --git"))]`
/// - After Down (move cursor to the next worktree): just wait for the screen to settle.
/// - After Alt-3 (switch to the main…± diff panel): wait until "diff --git" appears.
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

    // Helper to drain available output from the channel (non-blocking)
    let drain_output = |rx: &mpsc::Receiver<Vec<u8>>, parser: &mut vt100::Parser| {
        while let Ok(chunk) = rx.try_recv() {
            parser.process(&chunk);
        }
    };

    // Send each input and wait for screen to stabilize after each
    for (input, expected_content) in inputs {
        send_input_awaiting_content(&writer, &rx, &mut parser, input, *expected_content);
    }

    // Release the main thread's writer handle. The reader thread holds the
    // other Arc clone until the PTY drains, so this no longer drives stdin EOF.
    // The picker exits on Accept/Escape, or the kill below.
    drop(writer);

    // Poll for process exit (fast polling, long timeout for CI)
    let start = std::time::Instant::now();
    let timeout = CHILD_EXIT_TIMEOUT;
    while start.elapsed() < timeout {
        if child.try_wait().unwrap().is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let _ = child.kill(); // Kill if still running after timeout

    // Drain any remaining output
    drain_output(&rx, &mut parser);

    let exit_status = child.wait().unwrap();
    let exit_code = exit_status.exit_code() as i32;

    PtyResult { parser, exit_code }
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
    exec_in_pty_capture_before_abort_inner(
        command,
        args,
        working_dir,
        env_vars,
        pre_abort_inputs,
        false,
    )
}

/// Like `exec_in_pty_capture_before_abort`, but additionally waits for every
/// asynchronously-decorated list-pane column (git ahead/behind, CI status) to
/// resolve — its `·` loading placeholder cleared — before capturing.
///
/// Committed snapshots of the settled list frame must use this.
/// `wait_for_stable` alone gates on "no screen change for STABLE_DURATION plus
/// the awaited row text present", which a column's async output can satisfy
/// *inside a lull*: a slow git ahead/behind or CI-status compute (e.g. under
/// coverage instrumentation on a loaded Windows runner) emits nothing during
/// the window, so the screen looks stable while a cell still shows `·` where
/// the snapshot has the resolved value. That freezes a different async-render
/// frame than the committed snapshot — the exact race documented on
/// `exec_in_pty_capture_noop_probe`. No committed `switch_picker` snapshot
/// contains `·`, so the settled frame is always reachable.
///
/// Tests that *intentionally* capture a transient frame keep the non-settling
/// `exec_in_pty_capture_before_abort` — e.g.
/// `test_switch_picker_prs_shows_loading_marker`, which asserts the "loading
/// open PRs…" header is on screen before the rows stream in.
fn exec_in_pty_capture_settled_before_abort(
    command: &str,
    args: &[&str],
    working_dir: &Path,
    env_vars: &[(String, String)],
    pre_abort_inputs: &[(&str, Option<&str>)],
) -> PtyResult {
    exec_in_pty_capture_before_abort_inner(
        command,
        args,
        working_dir,
        env_vars,
        pre_abort_inputs,
        true,
    )
}

fn exec_in_pty_capture_before_abort_inner(
    command: &str,
    args: &[&str],
    working_dir: &Path,
    env_vars: &[(String, String)],
    pre_abort_inputs: &[(&str, Option<&str>)],
    settle_columns: bool,
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

    // The rows are on screen, but their async-decorated columns may still show
    // the `·` loading placeholder. A committed-snapshot caller waits for those
    // to resolve so it freezes the settled frame, not a loading one.
    if settle_columns {
        wait_for_list_columns_settled(&rx, &mut parser);
    }

    // === CAPTURE: screen state is now stable — snapshot BEFORE aborting ===
    // The parser retains this state because we stop feeding output to it.
    let exit_code = abort_and_exit_code(child, writer, rx);

    PtyResult { parser, exit_code }
}

/// Wait until every asynchronously-decorated list-pane column has resolved — no
/// `·` loading placeholder remains anywhere on screen.
///
/// `·` loading placeholders live in the list pane's column gutter, and no
/// committed `switch_picker` snapshot contains one, so a full-screen check
/// settles correctly for callers whose preview pane never renders a literal
/// `·` — e.g. `--branches` (commit-log preview). The `--prs` comments preview
/// renders `@author · {when}`, so a `--prs` caller can't use this full-screen
/// gate as-is (it would never clear, panicking at the stabilize timeout).
/// Routed through `wait_for_stable_until`'s
/// readiness predicate, so a timeout that never saw the placeholders clear
/// panics with diagnostics rather than silently capturing a loading frame.
fn wait_for_list_columns_settled(rx: &mpsc::Receiver<Vec<u8>>, parser: &mut vt100::Parser) {
    wait_for_stable_until(
        rx,
        parser,
        |screen| !screen.contains('·'),
        Some("all list-pane loading placeholders (·) to clear"),
        None,
    );
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
/// — observed as `test_switch_picker_worktree_row_comments_tab_shows_thread`
/// timing out with the cursor stuck on `main`, so the HEAD± tab showed the
/// primary's (empty) diff and the awaited `diff --git` never appeared. Re-issuing
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

    // Skim's previewer overlays its vertical scroll indicator (`{vscroll_offset}/
    // {content.len()}`) at the right edge of the preview pane's first line, in
    // reverse video — see `skim::previewer::Previewer::draw`. We don't see the
    // reverse-video attribute (vt100's `rows()` strips it), so it lands on screen
    // as bare `N/M` overlapping the tab header text. content.len() varies with
    // terminal width and preview content height, so it must be normalized.
    //
    // The previewer right-aligns the indicator at `screen_width - len - 1`, so
    // it overwrites a variable-width chunk at the right edge of the tab bar. With
    // all six numbered tabs the bar fills the 60-col preview pane, so the chunk
    // covers tab 6 (`6: pr`), its ` | ` divider, and a few trailing chars of tab
    // 5 — how many depends on the indicator's digit count (`5: summary1/46` vs
    // `5: summar1/286`). Anchor on the always-visible left portion (through
    // `5: summ`, well inside the pane) and rewrite the corrupted tail to the
    // canonical full bar. The exact per-tab styling (bold/plain/dim) is asserted
    // by the `items.rs` unit snapshots; here we only need a stable marker that
    // the bar rendered with tab 6 present.
    settings.add_filter(
        r"(?m)^(1: HEAD± \| 2: log \| 3: main…± \| 4: remote⇅ \| 5: summ).*$",
        "${1}ary | 6: pr [N/M]",
    );

    // Commit hashes (7-8 hex chars)
    settings.add_filter(r"\b[0-9a-f]{7,8}\b", "[HASH]");

    // Truncated commit hashes (6+ hex chars followed by ..) in narrow columns
    settings.add_filter(r"\b[0-9a-f]{6,8}\.\.", "[HASH]..");

    // Relative timestamps (1d, 16h, etc.)
    settings.add_filter(r"\b\d+[dhms]\b", "[TIME]");

    settings
}

#[rstest]
fn test_switch_picker_abort_with_escape(mut repo: TestRepo) {
    repo.remove_fixture_worktrees();
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

#[rstest]
fn test_switch_picker_with_multiple_worktrees(mut repo: TestRepo) {
    repo.remove_fixture_worktrees();
    // Remove origin so snapshots don't show origin/main
    repo.run_git(&["remote", "remove", "origin"]);
    repo.add_worktree("feature-one");
    repo.add_worktree("feature-two");

    let env_vars = repo.test_env_vars();
    let result = exec_in_pty_capture_before_abort(
        wt_bin().to_str().unwrap(),
        &["switch"],
        repo.root_path(),
        &env_vars,
        // Wait for items to render before capturing (prevents flakiness on slow CI)
        &[("", Some("feature-two"))],
    );

    assert_valid_abort_exit_code(result.exit_code);

    let (list, preview) = result.panels();
    let settings = switch_picker_settings(&repo);
    settings.bind(|| {
        assert_snapshot!("switch_picker_multiple_worktrees_list", list);
        assert_snapshot!("switch_picker_multiple_worktrees_preview", preview);
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
    repo.remove_fixture_worktrees();
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
    repo.remove_fixture_worktrees();
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

#[rstest]
fn test_switch_picker_with_branches(mut repo: TestRepo) {
    repo.remove_fixture_worktrees();
    // Remove origin so snapshots don't show origin/main
    repo.run_git(&["remote", "remove", "origin"]);
    repo.add_worktree("active-worktree");
    // Create a branch without a worktree
    let output = repo
        .git_command()
        .args(["branch", "orphan-branch"])
        .run()
        .unwrap();
    assert!(output.status.success(), "Failed to create branch");

    let env_vars = repo.test_env_vars();
    // Snapshot captures the settled column values (git ahead/behind), so wait
    // for the `·` loading placeholders to clear, not just for the row text:
    // under coverage instrumentation on a loaded Windows runner the stat
    // compute can lag past wait_for_stable's window, freezing a `···` loading
    // frame instead of the committed settled one (run 28213021257).
    let result = exec_in_pty_capture_settled_before_abort(
        wt_bin().to_str().unwrap(),
        &["switch", "--branches"],
        repo.root_path(),
        &env_vars,
        // Wait for branch items to render before capturing. On macOS CI under
        // heavy load, skim may show the prompt and header before item rows,
        // causing wait_for_stable to capture too early (just the header).
        &[("", Some("orphan-branch"))],
    );

    assert_valid_abort_exit_code(result.exit_code);

    let (list, preview) = result.panels();
    let settings = switch_picker_settings(&repo);
    settings.bind(|| {
        assert_snapshot!("switch_picker_with_branches_list", list);
        assert_snapshot!("switch_picker_with_branches_preview", preview);
    });
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
fn test_switch_picker_scrollbar_on_overflow(mut repo: TestRepo) {
    repo.remove_fixture_worktrees();
    repo.run_git(&["remote", "remove", "origin"]);

    // Far more branches than the ~24 item rows the 30-row terminal can show, so
    // the list is guaranteed to overflow and skim paints the scrollbar.
    for i in 0..50 {
        let name = format!("scroll-{i:02}");
        repo.run_git(&["branch", name.as_str()]);
    }

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

/// Typing a gutter glyph filters the picker by row kind. `+` is the linked-
/// worktree glyph, so it narrows to linked worktrees — excluding the current
/// worktree (`@`) and branch rows (`/`). This is the end-to-end answer to "why
/// doesn't `+` select *all* the worktrees?": the current worktree is a
/// different gutter kind. The `active-worktree` row has no literal `+` in its
/// name or path, so the match succeeds *only* via the glyph folded into the
/// search text (`progressive_handler::on_skeleton`) — if the fold regressed,
/// the list would empty out and the `active-worktree` wait below would time out.
#[rstest]
fn test_switch_picker_gutter_glyph_filters_by_kind(mut repo: TestRepo) {
    repo.remove_fixture_worktrees();
    // Remove origin so a remote-tracking row doesn't join the list.
    repo.run_git(&["remote", "remove", "origin"]);
    repo.add_worktree("active-worktree");
    // Create a branch without a worktree (a `/` gutter row).
    let output = repo
        .git_command()
        .args(["branch", "orphan-branch"])
        .run()
        .unwrap();
    assert!(output.status.success(), "Failed to create branch");

    let env_vars = repo.test_env_vars();
    let result = exec_in_pty_capture_before_abort(
        wt_bin().to_str().unwrap(),
        &["switch", "--branches"],
        repo.root_path(),
        &env_vars,
        // Type the linked-worktree glyph, then wait for the filtered list to
        // settle on the one linked worktree before capturing.
        &[("+", Some("active-worktree"))],
    );

    assert_valid_abort_exit_code(result.exit_code);

    let (list, _preview) = result.panels();
    assert!(
        list.contains("active-worktree"),
        "`+` keeps the linked worktree:\n{list}"
    );
    assert!(
        !list.contains("@ main"),
        "`+` filters out the current worktree (`@`):\n{list}"
    );
    assert!(
        !list.contains("orphan-branch"),
        "`+` filters out branch rows (`/`):\n{list}"
    );
}

#[rstest]
fn test_switch_picker_preview_panel_uncommitted(mut repo: TestRepo) {
    repo.remove_fixture_worktrees();
    // Remove origin so snapshots don't show origin/main
    repo.run_git(&["remote", "remove", "origin"]);
    let feature_path = repo.add_worktree("feature");

    // First, create and commit a file so we have something to modify
    std::fs::write(feature_path.join("tracked.txt"), "Original content\n").unwrap();
    let output = repo
        .git_command()
        .args(["-C", feature_path.to_str().unwrap(), "add", "tracked.txt"])
        .run()
        .unwrap();
    assert!(output.status.success(), "Failed to add file");
    let output = repo
        .git_command()
        .args([
            "-C",
            feature_path.to_str().unwrap(),
            "commit",
            "-m",
            "Add tracked file",
        ])
        .run()
        .unwrap();
    assert!(output.status.success(), "Failed to commit");

    // Now make uncommitted modifications to the tracked file
    std::fs::write(
        feature_path.join("tracked.txt"),
        "Modified content\nNew line added\nAnother line\n",
    )
    .unwrap();

    let env_vars = repo.test_env_vars();
    // Select `feature`, then Alt-1 for the HEAD± panel (bare digits filter; the
    // preview tabs moved to Alt). Wait for "diff --git" — the async preview can
    // be slow under congestion.
    let result = exec_in_pty_capture_before_abort(
        wt_bin().to_str().unwrap(),
        &["switch"],
        repo.root_path(),
        &env_vars,
        &[
            // Select `feature` by cursor navigation, not a filter query. skim's
            // matcher runs on a separate thread; under heavy parallel macOS load
            // its row redraw can lag the keystroke-driven prompt by more than the
            // 30s gate timeout (#2334/#2729/#2767). Arrow navigation never invokes
            // the matcher, so the selection is deterministic regardless of load.
            // The list now shows both worktrees with the cursor on `feature`; the
            // panel-content gate below still fails loudly if the wrong row is
            // selected (the snapshot would not match `feature`'s preview).
            ("\x1b[B", None),              // Down: move cursor to `feature`
            ("\x1b1", Some("diff --git")), // Alt-1: HEAD± panel; wait for diff
        ],
    );

    assert_valid_abort_exit_code(result.exit_code);

    let (list, preview) = result.panels();
    let settings = switch_picker_settings(&repo);
    settings.bind(|| {
        assert_snapshot!("switch_picker_preview_uncommitted_list", list);
        assert_snapshot!("switch_picker_preview_uncommitted_preview", preview);
    });
}

#[rstest]
fn test_switch_picker_preview_panel_log(mut repo: TestRepo) {
    repo.remove_fixture_worktrees();
    // Remove origin so snapshots don't show origin/main
    repo.run_git(&["remote", "remove", "origin"]);
    let feature_path = repo.add_worktree("feature");

    // Make several commits in the feature worktree
    for i in 1..=5 {
        std::fs::write(
            feature_path.join(format!("file{i}.txt")),
            format!("Content for file {i}\n"),
        )
        .unwrap();
        let output = repo
            .git_command()
            .args(["-C", feature_path.to_str().unwrap(), "add", "."])
            .run()
            .unwrap();
        assert!(output.status.success(), "Failed to add files");
        let output = repo
            .git_command()
            .args([
                "-C",
                feature_path.to_str().unwrap(),
                "commit",
                "-m",
                &format!("Add file {i} with content"),
            ])
            .run()
            .unwrap();
        assert!(output.status.success(), "Failed to commit");
    }

    let env_vars = repo.test_env_vars();
    // Select `feature`, then Alt-2 for the log panel. Wait for commit log
    // format "* [hash]" — the async preview can be slow under congestion.
    let result = exec_in_pty_capture_before_abort(
        wt_bin().to_str().unwrap(),
        &["switch"],
        repo.root_path(),
        &env_vars,
        &[
            // Cursor-navigation select: see test_switch_picker_preview_panel_uncommitted
            // for the matcher-lag rationale.
            ("\x1b[B", None),      // Down: move cursor to `feature`
            ("\x1b2", Some("* ")), // Alt-2: log panel; wait for git log output
        ],
    );

    assert_valid_abort_exit_code(result.exit_code);

    let (list, preview) = result.panels();
    let settings = switch_picker_settings(&repo);
    settings.bind(|| {
        assert_snapshot!("switch_picker_preview_log_list", list);
        assert_snapshot!("switch_picker_preview_log_preview", preview);
    });
}

#[rstest]
fn test_switch_picker_preview_panel_main_diff(mut repo: TestRepo) {
    repo.remove_fixture_worktrees();
    // Remove origin so snapshots don't show origin/main
    repo.run_git(&["remote", "remove", "origin"]);
    let feature_path = repo.add_worktree("feature");

    // Make commits in the feature worktree that differ from main
    std::fs::write(
        feature_path.join("feature_code.rs"),
        r#"fn new_feature() {
    println!("This is a new feature!");
    let x = 42;
    let y = x * 2;
    println!("Result: {}", y);
}
"#,
    )
    .unwrap();
    let output = repo
        .git_command()
        .args(["-C", feature_path.to_str().unwrap(), "add", "."])
        .run()
        .unwrap();
    assert!(output.status.success(), "Failed to add files");
    let output = repo
        .git_command()
        .args([
            "-C",
            feature_path.to_str().unwrap(),
            "commit",
            "-m",
            "Add new feature implementation",
        ])
        .run()
        .unwrap();
    assert!(output.status.success(), "Failed to commit");

    // Add another commit
    std::fs::write(
        feature_path.join("tests.rs"),
        r#"#[test]
fn test_new_feature() {
    assert_eq!(42 * 2, 84);
}
"#,
    )
    .unwrap();
    let output = repo
        .git_command()
        .args(["-C", feature_path.to_str().unwrap(), "add", "."])
        .run()
        .unwrap();
    assert!(output.status.success(), "Failed to add files");
    let output = repo
        .git_command()
        .args([
            "-C",
            feature_path.to_str().unwrap(),
            "commit",
            "-m",
            "Add tests for new feature",
        ])
        .run()
        .unwrap();
    assert!(output.status.success(), "Failed to commit");

    let env_vars = repo.test_env_vars();
    // Select `feature`, then Alt-3 for the main…± panel. Wait for "diff --git"
    // — the async preview can be slow under congestion.
    let result = exec_in_pty_capture_before_abort(
        wt_bin().to_str().unwrap(),
        &["switch"],
        repo.root_path(),
        &env_vars,
        &[
            // Cursor-navigation select: see test_switch_picker_preview_panel_uncommitted
            // for the matcher-lag rationale.
            ("\x1b[B", None),              // Down: move cursor to `feature`
            ("\x1b3", Some("diff --git")), // Alt-3: main…± panel; wait for diff
        ],
    );

    assert_valid_abort_exit_code(result.exit_code);

    let (list, preview) = result.panels();
    let settings = switch_picker_settings(&repo);
    settings.bind(|| {
        assert_snapshot!("switch_picker_preview_main_diff_list", list);
        assert_snapshot!("switch_picker_preview_main_diff_preview", preview);
    });
}

#[rstest]
fn test_switch_picker_preview_panel_summary(mut repo: TestRepo) {
    repo.remove_fixture_worktrees();
    // Remove origin so snapshots don't show origin/main
    repo.run_git(&["remote", "remove", "origin"]);
    let feature_path = repo.add_worktree("feature");

    // Make a commit so there's content to potentially summarize
    std::fs::write(feature_path.join("new.txt"), "content\n").unwrap();
    let output = repo
        .git_command()
        .args(["-C", feature_path.to_str().unwrap(), "add", "."])
        .run()
        .unwrap();
    assert!(output.status.success(), "Failed to add file");
    let output = repo
        .git_command()
        .args([
            "-C",
            feature_path.to_str().unwrap(),
            "commit",
            "-m",
            "Add new file",
        ])
        .run()
        .unwrap();
    assert!(output.status.success(), "Failed to commit");

    let env_vars = repo.test_env_vars();
    // Select `feature`, then Alt-5 for the summary panel. Wait for the
    // "commit.generation" config hint since no LLM is configured.
    let result = exec_in_pty_capture_before_abort(
        wt_bin().to_str().unwrap(),
        &["switch"],
        repo.root_path(),
        &env_vars,
        &[
            // Cursor-navigation select: see test_switch_picker_preview_panel_uncommitted
            // for the matcher-lag rationale.
            ("\x1b[B", None),                     // Down: move cursor to `feature`
            ("\x1b5", Some("commit.generation")), // Alt-5: summary panel; wait for hint
        ],
    );

    assert_valid_abort_exit_code(result.exit_code);

    let (list, preview) = result.panels();
    let settings = switch_picker_settings(&repo);
    settings.bind(|| {
        assert_snapshot!("switch_picker_preview_summary_list", list);
        assert_snapshot!("switch_picker_preview_summary_preview", preview);
    });
}

/// The summary panel's "enable summaries" hint — shown when commit.generation
/// IS configured but `[list] summary = false`. Covers the second hint arm (the
/// first is exercised by `test_switch_picker_preview_panel_summary`), and pins
/// that both hints point at the resolved config path: under the test's
/// `WORKTRUNK_CONFIG_PATH` isolation that path redacts to `[TEST_CONFIG]`, not
/// a hardcoded `~/.config/worktrunk/config.toml`.
#[rstest]
fn test_switch_picker_preview_panel_summary_disabled(mut repo: TestRepo) {
    repo.remove_fixture_worktrees();
    // Remove origin so snapshots don't show origin/main
    repo.run_git(&["remote", "remove", "origin"]);
    repo.add_worktree("feature");

    // commit.generation configured, but summaries off — the picker seeds the
    // Summary tab with the "enable summaries" hint rather than generating one.
    repo.write_test_config(
        "[commit.generation]\ncommand = \"llm -m haiku\"\n\n[list]\nsummary = false\n",
    );

    let env_vars = repo.test_env_vars();
    // Select `feature`, then Alt-5 for the summary panel. Wait for the hint.
    let result = exec_in_pty_capture_before_abort(
        wt_bin().to_str().unwrap(),
        &["switch"],
        repo.root_path(),
        &env_vars,
        &[
            // Cursor-navigation select: see test_switch_picker_preview_panel_uncommitted
            // for the matcher-lag rationale.
            ("\x1b[B", None),          // Down: move cursor to `feature`
            ("\x1b5", Some("Enable")), // Alt-5: summary panel; wait for hint
        ],
    );

    assert_valid_abort_exit_code(result.exit_code);

    let (_, preview) = result.panels();
    let settings = switch_picker_settings(&repo);
    settings.bind(|| {
        assert_snapshot!("switch_picker_preview_summary_disabled_preview", preview);
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

/// Install a mock forge CLI (`gh`/`glab`) that answers the `--prs` list call
/// from a canned JSON file, and return env vars (mock on PATH + MOCK_CONFIG_DIR)
/// for a `wt switch --prs` PTY run. No network is touched. `list_delay_ms`
/// sleeps the list call (0 = instant) so a test can observe the picker's
/// in-flight loading marker before the rows land.
fn mock_forge_env(
    repo: &TestRepo,
    cli: &str,
    list_cmd: &str,
    list_json: &str,
    list_delay_ms: u64,
) -> Vec<(String, String)> {
    let mock_bin = repo.root_path().join("mock-bin");
    std::fs::create_dir_all(&mock_bin).unwrap();
    std::fs::write(mock_bin.join("list.json"), list_json).unwrap();
    MockConfig::new(cli)
        .version(&format!("{cli} version 1.0.0 (mock)"))
        .command(
            list_cmd,
            MockResponse::file("list.json").with_delay_ms(list_delay_ms),
        )
        .command("_default", MockResponse::exit(1))
        .write(&mock_bin);

    forge_mock_env_vars(repo, &mock_bin)
}

/// Env vars (mock-bin on PATH + `MOCK_CONFIG_DIR`) for a PTY `wt` run that should
/// resolve `gh`/`glab` to a mock written into `mock_bin`. Shared by
/// [`mock_forge_env`] and tests that build a richer mock (extra `pr view`
/// responses) directly.
fn forge_mock_env_vars(repo: &TestRepo, mock_bin: &Path) -> Vec<(String, String)> {
    let mut env_vars = repo.test_env_vars();
    env_vars.push((
        "MOCK_CONFIG_DIR".to_string(),
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

/// `wt switch --prs` on a GitHub repo: the open-PR list streams into the picker
/// via a mocked `gh pr list`. Asserts the PR row reaches the list (the `#42`
/// reference in the CI column), which deterministically exercises the whole
/// fetch → stream → render path (`fetch_open_prs`, `fetch_github`,
/// `parse_github_prs`, `stream_open_prs`, `PrEntry::display_status`, `render_grid_row`,
/// `render_pr_description`). The title isn't on the row — it lives in the `pr`
/// preview tab so the columns align — so the row's stable substring is `#42`.
/// The full list isn't snapshotted because the worktree rows' CI cells fill
/// asynchronously.
#[rstest]
fn test_switch_picker_prs_github_list(mut repo: TestRepo) {
    repo.remove_fixture_worktrees();
    repo.run_git(&[
        "remote",
        "set-url",
        "origin",
        "https://github.com/owner/test-repo.git",
    ]);
    let pr_json = r#"[{"number":42,"title":"Retry the flaky network test","headRefName":"fix/flaky","author":{"login":"octocat"},"isDraft":false,"url":"https://github.com/owner/test-repo/pull/42","body":"Wraps the request in a retry so the suite stops flaking."}]"#;
    let env_vars = mock_forge_env(&repo, "gh", "pr list", pr_json, 0);

    let result = exec_in_pty_capture_before_abort(
        wt_bin().to_str().unwrap(),
        &["switch", "--prs"],
        repo.root_path(),
        &env_vars,
        // `main…±` now occupies the preview-shown list pane, clipping the CI
        // column (`#42`) past skim's split. Toggle the preview off (alt-p) so
        // the list spans full width and renders the number, then gate on it —
        // deterministic, no dependency on selecting an async-arrived row.
        &[("\x1bp", Some("#42"))],
    );

    assert_valid_abort_exit_code(result.exit_code);
    // Preview off → full-width list renders the CI column; assert on the whole
    // screen since `#42` sits past the panel split column.
    let screen = result.screen();
    assert!(screen.contains("#42"), "PR number on screen:\n{screen}");
    // The title lives in the preview (now hidden), never on the row itself.
    assert!(
        !screen.contains("Retry the flaky network test"),
        "PR title should stay off the row:\n{screen}"
    );
    // The header's loading marker is gone once the rows have streamed in.
    assert!(
        !screen.contains("Loading open PRs"),
        "loading marker cleared once rows arrived:\n{screen}"
    );
}

/// `wt switch --prs` on a GitLab repo: the open-MR list streams in via a mocked
/// `glab mr list`. Covers the GitLab fetch path (`fetch_gitlab`,
/// `parse_gitlab_mrs`, `gitlab_mr_status`).
#[rstest]
fn test_switch_picker_prs_gitlab_list(mut repo: TestRepo) {
    repo.remove_fixture_worktrees();
    repo.run_git(&[
        "remote",
        "set-url",
        "origin",
        "https://gitlab.com/owner/test-repo.git",
    ]);
    let mr_json = r#"[{"iid":7,"title":"Cache the dependency graph","source_branch":"feat/cache","author":{"username":"alice"},"draft":false,"web_url":"https://gitlab.com/owner/test-repo/-/merge_requests/7","detailed_merge_status":"ci_still_running","description":"Speeds up CI by caching deps between jobs."}]"#;
    let env_vars = mock_forge_env(&repo, "glab", "mr list", mr_json, 0);

    let result = exec_in_pty_capture_before_abort(
        wt_bin().to_str().unwrap(),
        &["switch", "--prs"],
        repo.root_path(),
        &env_vars,
        // `main…±` clips the CI column (`!7`) past skim's split in the
        // preview-shown pane. Toggle the preview off (alt-p) so the full-width
        // list renders the ref, then gate on it.
        &[("\x1bp", Some("!7"))],
    );

    assert_valid_abort_exit_code(result.exit_code);
    // Preview off → full-width list renders the CI column; assert on the whole
    // screen since `!7` sits past the panel split column.
    let screen = result.screen();
    assert!(screen.contains("!7"), "MR number on screen:\n{screen}");
    assert!(
        !screen.contains("Cache the dependency graph"),
        "MR title should stay off the row:\n{screen}"
    );
}

/// Removing a worktree row with alt-x in `--prs` mode must keep the streamed
/// PR/MR rows on screen — only the removed worktree row leaves the list, no
/// alt-r refresh needed.
///
/// alt-x's drop path rebuilds skim's item pool from the picker's shared row list
/// (`resync_pool`). The `--prs` thread streams its PR rows straight to skim's
/// item channel, so unless they're also recorded in that shared list the rebuild
/// drops them. This drives the drop path (a clean, integrated worktree, removed
/// row leaves the list) and asserts the `#42` PR row survives it.
#[rstest]
fn test_switch_picker_prs_rows_survive_alt_x_removal(mut repo: TestRepo) {
    repo.remove_fixture_worktrees();
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
        .command("pr list --state", MockResponse::file("list.json"))
        .command("pr list --head", MockResponse::output("[]"))
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

    // Preview off (alt-p) so the full-width list renders the CI column (`#42`),
    // which the preview-shown pane otherwise clips past skim's split. The wait
    // also blocks until the `--prs` row has streamed in.
    send_input_awaiting_content(&writer, &rx, &mut parser, "\x1bp", Some("#42"));
    // The worktree row is on screen too (one skeleton batch).
    wait_for_stable_with_content(&rx, &mut parser, Some("wt-drop"));

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

/// alt-x on a row that can't be removed flashes the reason in the header at
/// alt-x time, not only when the stash drains on exit. The current worktree is
/// the picker's pinned top row, so launching from the main worktree and pressing
/// alt-x (no cursor move) targets it — and the main worktree can't be removed, so
/// the header swaps the column labels for `▲ The main worktree cannot be removed`
/// for a beat. The flash *clearing* after the beat is covered by the `HeaderFlash`
/// unit tests; this asserts it paints. A presence-wait catches it before the beat
/// elapses (`HEADER_FLASH_DURATION`), so the test never depends on the timer.
#[rstest]
fn test_switch_picker_alt_x_flashes_unremovable_reason(mut repo: TestRepo) {
    repo.remove_fixture_worktrees();
    repo.run_git(&["remote", "remove", "origin"]);
    // A second worktree so the skeleton paints a distinctive row to gate on; the
    // cursor still starts on the pinned current (main) worktree above it.
    repo.add_worktree("wt-extra");

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

    // The worktree rows have painted (one skeleton batch); the cursor is on the
    // pinned current worktree — the main worktree, which can't be removed.
    wait_for_stable_with_content(&rx, &mut parser, Some("wt-extra"));

    // alt-x is declined, and the reason flashes in the header. The presence-wait
    // returns as soon as the flash paints — before it self-clears.
    send_input_awaiting_content(
        &writer,
        &rx,
        &mut parser,
        "\x1bx",
        Some("main worktree cannot be removed"),
    );

    let screen = parser.screen().contents();
    let exit_code = abort_and_exit_code(child, writer, rx);
    assert_valid_abort_exit_code(exit_code);
    assert!(
        screen.contains("main worktree cannot be removed"),
        "the unremovable reason flashes in the header at alt-x time:\n{screen}"
    );
}

/// A preview pane fills in on its own once its background compute lands — no
/// keystroke needed. The deterministic vehicle is a `--prs` row's `comments`
/// tab: the comment fetch (`gh pr view <n> --json comments`) is mocked behind a
/// delay, so when the row is selected and the comments tab is opened the pane is
/// still on its "Loading comments…" placeholder. The comment then surfaces with
/// no further input once the delayed fetch resolves and the orchestrator pokes a
/// repaint (`PreviewNotifier`). Before that product-side poke the placeholder
/// would strand until the next keystroke — the gap the picker's test harness
/// used to paper over by re-issuing the tab key.
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
fn test_switch_picker_preview_auto_refreshes_when_compute_lands(mut repo: TestRepo) {
    repo.remove_fixture_worktrees();
    repo.run_git(&[
        "remote",
        "set-url",
        "origin",
        "https://github.com/owner/test-repo.git",
    ]);

    // The mock holds `gh pr view` (the comments / log fetch) long enough that the
    // tab is reliably opened before the fetch resolves — so the pane is observed
    // mid-load and the appearance of the comment proves the auto-refresh, not a
    // cache hit. `pr list` is instant so the row lands promptly.
    let mock_bin = repo.root_path().join("mock-bin");
    std::fs::create_dir_all(&mock_bin).unwrap();
    // A short head branch so the PR row isn't truncated in the narrow
    // (preview-shown) list pane.
    let pr_json = r#"[{"number":42,"title":"Retry the flaky network test","headRefName":"flaky","author":{"login":"octocat"},"isDraft":false,"url":"https://github.com/owner/test-repo/pull/42","body":"body"}]"#;
    std::fs::write(mock_bin.join("pr_list.json"), pr_json).unwrap();
    let comments_json = r#"{"comments":[{"author":{"login":"octocat"},"body":"AUTOREFRESHMARK","createdAt":"2025-01-01T00:00:00Z"}]}"#;
    MockConfig::new("gh")
        .version("gh version 1.0.0 (mock)")
        .command("pr list", MockResponse::file("pr_list.json"))
        .command(
            "pr view",
            MockResponse::output(comments_json).with_delay_ms(3000),
        )
        .command("_default", MockResponse::exit(1))
        .write(&mock_bin);
    let env_vars = forge_mock_env_vars(&repo, &mock_bin);

    let result = exec_in_pty_capture_before_abort(
        wt_bin().to_str().unwrap(),
        &["switch", "--prs"],
        repo.root_path(),
        &env_vars,
        &[
            // Drive the cursor onto the PR row. Its narrow (preview-shown) list
            // line shows the short head branch `flaky`; the worktree row shows
            // `main`. A re-issued Down lands the `>` pointer on `flaky` (the
            // bottom row) and re-drives it there if a streamed-row refresh resets
            // the cursor — see the docstring for why this replaces the `!main`
            // filter.
            ("\x1b[B", Some("flaky")),
            // alt-7: comments tab. The fetch is still in flight, so the pane shows
            // "Loading comments…"; the comment appears with NO further input once
            // the delayed fetch lands and the picker repaints on its own.
            ("\x1b7", Some("AUTOREFRESHMARK")),
        ],
    );

    assert_valid_abort_exit_code(result.exit_code);
    let (_list, preview) = result.panels();
    assert!(
        preview.contains("AUTOREFRESHMARK"),
        "comment surfaced on its own once the delayed fetch landed:\n{preview}"
    );
}

/// `wt switch --prs` shows a dim "↳ Loading open PRs…" marker on the header row
/// while the forge call is in flight. A delayed mock holds the PR list long
/// enough to observe the marker on the real screen before the rows land. The
/// picker captures and aborts at stabilize time — well before the delay
/// elapses — and the `--prs` thread is detached on exit (not joined), so the
/// test never pays the full delay. The marker's *clearing* once rows arrive is
/// covered by the `header_loading_marker_shows_until_cleared` unit test and the
/// negative assertion in `test_switch_picker_prs_github_list`.
#[rstest]
fn test_switch_picker_prs_shows_loading_marker(mut repo: TestRepo) {
    repo.remove_fixture_worktrees();
    repo.run_git(&[
        "remote",
        "set-url",
        "origin",
        "https://github.com/owner/test-repo.git",
    ]);
    let pr_json = r#"[{"number":42,"title":"Retry the flaky network test","headRefName":"fix/flaky","author":{"login":"octocat"},"isDraft":false,"url":"https://github.com/owner/test-repo/pull/42","body":""}]"#;
    // 3s delay >> the ~1s capture, so the marker is still on screen when the
    // helper snapshots and aborts.
    let env_vars = mock_forge_env(&repo, "gh", "pr list", pr_json, 3000);

    let result = exec_in_pty_capture_before_abort(
        wt_bin().to_str().unwrap(),
        &["switch", "--prs"],
        repo.root_path(),
        &env_vars,
        // The loading line paints at skeleton, before the (slow) forge call
        // returns its rows.
        &[("", Some("Loading open PRs"))],
    );

    assert_valid_abort_exit_code(result.exit_code);
    let (list, _preview) = result.panels();
    assert!(
        list.contains("Loading open PRs"),
        "loading line on the header while --prs fetches:\n{list}"
    );
    // The PR row hasn't streamed in yet — still inside the delayed forge call.
    assert!(!list.contains("#42"), "rows not yet streamed in:\n{list}");
}

/// A plain `wt switch` (no `--prs`) worktree row whose branch has an open PR
/// shows the real comment thread in its `comments` tab — the same background
/// `gh pr view <n> --json comments` fetch and render a `--prs` row makes. This
/// is the crux of the unification: the comments tab no longer points a worktree
/// row at `--prs`; it loads the thread directly. The PR is primed from a fresh
/// CI cache so the row resolves to "has PR" at skeleton with no `gh pr list`,
/// and the conversation comes from a mocked `gh pr view`.
#[rstest]
fn test_switch_picker_worktree_row_comments_tab_shows_thread(mut repo: TestRepo) {
    repo.remove_fixture_worktrees();
    repo.run_git(&[
        "remote",
        "set-url",
        "origin",
        "https://github.com/owner/test-repo.git",
    ]);
    let feature_path = repo.add_worktree("feature");
    // Commit a file, then leave an uncommitted edit so `feature`'s HEAD± tab
    // shows a `diff --git` — a preview-only sync anchor (it never appears in the
    // list) that confirms the cursor landed on `feature` before reading comments.
    std::fs::write(feature_path.join("file.txt"), "content\n").unwrap();
    let add = repo
        .git_command()
        .args(["-C", feature_path.to_str().unwrap(), "add", "."])
        .run()
        .unwrap();
    assert!(add.status.success(), "Failed to add file");
    let commit = repo
        .git_command()
        .args([
            "-C",
            feature_path.to_str().unwrap(),
            "commit",
            "-m",
            "Commit for comments tab",
        ])
        .run()
        .unwrap();
    assert!(commit.status.success(), "Failed to commit");
    std::fs::write(feature_path.join("file.txt"), "content\nmore\n").unwrap();

    // Prime a fresh CI status carrying an open PR (#42) so the row resolves to
    // "has PR" at skeleton — detect reads this same fresh entry, so no `gh pr
    // list`. The comments thread still needs a live `gh pr view`, mocked below.
    seed_ci_status(
        &repo,
        "feature",
        r##"{"ci_status":"passed","source":"pr","is_stale":false,"number":{"number":42,"sigil":"#"}}"##,
    );

    // Mock the only forge call a worktree row makes — `gh pr view 42 --json
    // comments` (the `log` tab is local; the `pr` tab rides the cached CI). Any
    // other gh invocation falls through to `_default` exit 1.
    let mock_bin = repo.root_path().join("mock-bin");
    std::fs::create_dir_all(&mock_bin).unwrap();
    let comments_json = r#"{"comments":[{"author":{"login":"octocat"},"body":"Looks solid, shipping it.","createdAt":"2024-12-01T00:00:00Z"}]}"#;
    std::fs::write(mock_bin.join("comments.json"), comments_json).unwrap();
    MockConfig::new("gh")
        .version("gh version 1.0.0 (mock)")
        .command("pr view", MockResponse::file("comments.json"))
        .command("_default", MockResponse::exit(1))
        .write(&mock_bin);
    let mut env_vars = repo.test_env_vars();
    env_vars.push((
        "MOCK_CONFIG_DIR".to_string(),
        mock_bin.display().to_string(),
    ));
    // Prepend mock-bin to PATH using the OS separator (`;` on Windows, `:` on
    // Unix) — a hardcoded `:` corrupts the PATH on Windows, so the mock `gh.exe`
    // is never found and the comments fetch fails ("Couldn't load comments").
    let base_path = std::env::var_os("PATH").unwrap_or_default();
    let mut paths = vec![mock_bin.clone()];
    paths.extend(std::env::split_paths(&base_path));
    let joined = std::env::join_paths(paths).expect("mock-bin joins into PATH");
    env_vars.push(("PATH".to_string(), joined.to_string_lossy().into_owned()));

    let result = exec_in_pty_capture_before_abort(
        wt_bin().to_str().unwrap(),
        &["switch"],
        repo.root_path(),
        &env_vars,
        &[
            // Cursor-navigation select: see test_switch_picker_preview_panel_uncommitted
            // for the matcher-lag rationale. Gate on the row name so the Down is
            // re-issued until the `>` pointer lands on `feature` — this row
            // decorates asynchronously (primed CI status → "has PR"), and that
            // background refresh resets skim's cursor to the top, which stranded
            // the pointer on `main` and timed this test out on Windows CI.
            ("\x1b[B", Some("feature")), // Down: move cursor to `feature`
            // Alt-1: HEAD± panel. `feature`'s uncommitted diff (`diff --git`) is a
            // preview-only anchor that gives the skeleton-spawned comments fetch
            // time to land before we read its tab.
            ("\x1b1", Some("diff --git")),
            // Alt-7: jump to comments (7). The thread was fetched in the
            // background at skeleton time (the PR was primed), so it's cached by
            // now — wait for the comment body to confirm the real thread renders.
            ("\x1b7", Some("Looks solid")),
        ],
    );

    assert_valid_abort_exit_code(result.exit_code);
    let (_list, preview) = result.panels();
    assert!(
        preview.contains("octocat"),
        "comment author renders on a worktree row's comments tab:\n{preview}"
    );
    assert!(
        preview.contains("Looks solid"),
        "comment body renders on a worktree row's comments tab:\n{preview}"
    );
    // No `--prs` pointer — the worktree row loads the thread itself.
    assert!(
        !preview.contains("--prs"),
        "comments tab must not point at --prs:\n{preview}"
    );
}

/// The `pr` tab auto-resolves from "Fetching PR status…" to the live PR with no
/// keystroke. A worktree row's CI status is fetched live (`gh pr list --head`),
/// mocked behind a delay and left UNSEEDED so the tab is observed mid-fetch; when
/// the status lands, `on_update` pokes a `RunPreview` (`PreviewNotifier`) and the
/// pane re-renders. Distinct producer from
/// `test_switch_picker_preview_auto_refreshes_when_compute_lands` (an orchestrator
/// cache fill): here it's the CI fetch surfaced through `on_update`.
#[rstest]
fn test_switch_picker_pr_tab_auto_resolves_from_fetching(mut repo: TestRepo) {
    repo.remove_fixture_worktrees();
    repo.run_git(&[
        "remote",
        "set-url",
        "origin",
        "https://github.com/owner/test-repo.git",
    ]);

    let mock_bin = repo.root_path().join("mock-bin");
    std::fs::create_dir_all(&mock_bin).unwrap();
    // The per-row CI fetch (`gh pr list --head <branch>`), delayed so the `pr`
    // tab is opened while still "Fetching PR status…"; unseeded, so it starts
    // there rather than resolving at skeleton from the cache.
    let pr_list_json = r#"[{"number":654,"title":"PRTABMARK auto-resolve","body":"","comments":[],"statusCheckRollup":[],"url":"https://github.com/owner/test-repo/pull/654","isDraft":false}]"#;
    std::fs::write(mock_bin.join("pr_list.json"), pr_list_json).unwrap();
    MockConfig::new("gh")
        .version("gh version 1.0.0 (mock)")
        .command(
            "pr list",
            MockResponse::file("pr_list.json").with_delay_ms(3000),
        )
        .command("_default", MockResponse::exit(1))
        .write(&mock_bin);
    let env_vars = forge_mock_env_vars(&repo, &mock_bin);

    let result = exec_in_pty_capture_before_abort(
        wt_bin().to_str().unwrap(),
        &["switch"],
        repo.root_path(),
        &env_vars,
        &[
            // alt-6: the `pr` tab. The CI fetch is still in flight (delayed), so
            // the pane shows the fetching hint.
            ("\x1b6", Some("Fetching PR status")),
            // No further input: the resolved PR title appears on its own once the
            // delayed CI fetch lands and the picker repaints.
            ("", Some("PRTABMARK")),
        ],
    );

    assert_valid_abort_exit_code(result.exit_code);
    let (_list, preview) = result.panels();
    assert!(
        preview.contains("PRTABMARK"),
        "pr tab resolved from 'Fetching' on its own:\n{preview}"
    );
}

#[rstest]
fn test_switch_picker_preview_cycle_tab_forward(mut repo: TestRepo) {
    // Tab cycles the preview tab forward. From the default HEAD± tab (1), one
    // Tab lands on the log tab (2), exercising the native `PreviewMode::next`
    // rotation behind the tab key's `Action::Custom` binding. (alt-1..alt-7 jump
    // directly; the panel tests above cover those — this covers the cycle path.)
    repo.remove_fixture_worktrees();
    repo.run_git(&["remote", "remove", "origin"]);
    let feature_path = repo.add_worktree("feature");

    // Commit a file so the log tab has content; the worktree stays clean, so the
    // HEAD± tab shows no diff and the "* " log-graph marker is unambiguous.
    std::fs::write(feature_path.join("file.txt"), "content\n").unwrap();
    let add = repo
        .git_command()
        .args(["-C", feature_path.to_str().unwrap(), "add", "."])
        .run()
        .unwrap();
    assert!(add.status.success(), "Failed to add file");
    let commit = repo
        .git_command()
        .args([
            "-C",
            feature_path.to_str().unwrap(),
            "commit",
            "-m",
            "Commit for tab cycle",
        ])
        .run()
        .unwrap();
    assert!(commit.status.success(), "Failed to commit");

    let env_vars = repo.test_env_vars();
    let result = exec_in_pty_capture_before_abort(
        wt_bin().to_str().unwrap(),
        &["switch"],
        repo.root_path(),
        &env_vars,
        &[
            // Cursor-navigation select: see test_switch_picker_preview_panel_uncommitted
            // for the matcher-lag rationale.
            ("\x1b[B", None),   // Down: move cursor to `feature`
            ("\t", Some("* ")), // Tab: HEAD± → log; wait for git log output
        ],
    );

    assert_valid_abort_exit_code(result.exit_code);

    let (_list, preview) = result.panels();
    assert!(
        preview.contains("* "),
        "Tab should advance the preview to the log tab; preview was:\n{preview}"
    );
}

#[rstest]
fn test_switch_picker_preview_cycle_tab_forward_wraps(mut repo: TestRepo) {
    // Forward cycling wraps 7 → 1: from the comments tab (reached via Alt-7), one
    // Tab returns to the HEAD± tab. This covers the `7 → 1` wraparound in
    // `PreviewMode::next`, the half most easily typo'd away.
    repo.remove_fixture_worktrees();
    repo.run_git(&["remote", "remove", "origin"]);
    let feature_path = repo.add_worktree("feature");

    // Commit so the worktree is clean: the HEAD± tab then reads "no uncommitted
    // changes", a message unique to tab 1 — so seeing it proves the wrap landed
    // there and not on some other tab.
    std::fs::write(feature_path.join("file.txt"), "content\n").unwrap();
    let add = repo
        .git_command()
        .args(["-C", feature_path.to_str().unwrap(), "add", "."])
        .run()
        .unwrap();
    assert!(add.status.success(), "Failed to add file");
    let commit = repo
        .git_command()
        .args([
            "-C",
            feature_path.to_str().unwrap(),
            "commit",
            "-m",
            "Commit for forward-wrap cycle",
        ])
        .run()
        .unwrap();
    assert!(commit.status.success(), "Failed to commit");

    seed_ci_status(&repo, "feature", "null");

    let env_vars = repo.test_env_vars();
    let result = exec_in_pty_capture_before_abort(
        wt_bin().to_str().unwrap(),
        &["switch"],
        repo.root_path(),
        &env_vars,
        &[
            // Cursor-navigation select: see test_switch_picker_preview_panel_uncommitted
            // for the matcher-lag rationale.
            ("\x1b[B", None), // Down: move cursor to `feature`
            // Alt-7: jump to comments (7). The comments tab behaves the same on a
            // worktree row as on a `--prs` row — with no PR (seeded above) it shows
            // "has no PR", matching the `pr` tab.
            ("\x1b7", Some("has no PR")),
            ("\t", Some("no uncommitted changes")), // Tab: wrap 7 → HEAD± (1)
        ],
    );

    assert_valid_abort_exit_code(result.exit_code);

    let (_list, preview) = result.panels();
    assert!(
        preview.contains("no uncommitted changes"),
        "Tab from the comments tab should wrap to the HEAD± tab; preview was:\n{preview}"
    );
}

#[rstest]
fn test_switch_picker_preview_cycle_shift_tab_wraps(mut repo: TestRepo) {
    // Shift-Tab cycles backward and wraps: from the default HEAD± tab (1), one
    // Shift-Tab lands on the comments tab (7), exercising the reverse rotation
    // `PreviewMode::prev` including the 1 → 7 wraparound.
    repo.remove_fixture_worktrees();
    repo.run_git(&["remote", "remove", "origin"]);
    let feature_path = repo.add_worktree("feature");

    std::fs::write(feature_path.join("new.txt"), "content\n").unwrap();
    let add = repo
        .git_command()
        .args(["-C", feature_path.to_str().unwrap(), "add", "."])
        .run()
        .unwrap();
    assert!(add.status.success(), "Failed to add file");
    let commit = repo
        .git_command()
        .args([
            "-C",
            feature_path.to_str().unwrap(),
            "commit",
            "-m",
            "Add new file",
        ])
        .run()
        .unwrap();
    assert!(commit.status.success(), "Failed to commit");

    seed_ci_status(&repo, "feature", "null");

    let env_vars = repo.test_env_vars();
    let result = exec_in_pty_capture_before_abort(
        wt_bin().to_str().unwrap(),
        &["switch"],
        repo.root_path(),
        &env_vars,
        &[
            // Cursor-navigation select: see test_switch_picker_preview_panel_uncommitted
            // for the matcher-lag rationale.
            ("\x1b[B", None), // Down: move cursor to `feature`
            // Shift-Tab: HEAD± (1) → comments (7), the 1 → 7 wraparound. With no PR
            // (seeded above) the comments tab shows "has no PR", like the `pr` tab.
            ("\x1b[Z", Some("has no PR")),
        ],
    );

    assert_valid_abort_exit_code(result.exit_code);

    let (_list, preview) = result.panels();
    assert!(
        preview.contains("has no PR"),
        "Shift-Tab should wrap to the comments tab; preview was:\n{preview}"
    );
}

#[rstest]
fn test_switch_picker_respects_list_config(mut repo: TestRepo) {
    // Use the same reliable setup as test_switch_picker_with_branches:
    // remove fixture worktrees (which use relative gitdir paths that can fail
    // to resolve under concurrent operations) and origin (to avoid remote branch noise)
    repo.remove_fixture_worktrees();
    repo.run_git(&["remote", "remove", "origin"]);

    repo.add_worktree("active-worktree");
    // Create a branch without a worktree
    let output = repo
        .git_command()
        .args(["branch", "orphan-branch"])
        .run()
        .unwrap();
    assert!(output.status.success(), "Failed to create branch");

    // Write user config with [list] branches = true
    // This should enable branches in the picker without the --branches flag
    repo.write_test_config(
        r#"
[list]
branches = true
"#,
    );

    let env_vars = repo.test_env_vars();
    // Capture screen BEFORE sending Escape. Screen must stabilize with orphan-branch visible.
    let result = exec_in_pty_capture_before_abort(
        wt_bin().to_str().unwrap(),
        &["switch"], // No --branches flag - config should enable it
        repo.root_path(),
        &env_vars,
        &[("", Some("orphan-branch"))], // Wait for orphan-branch to appear in list before abort
    );

    assert_valid_abort_exit_code(result.exit_code);

    let screen = result.screen();
    // Verify that orphan-branch appears (enabled by config, not CLI flag)
    assert!(
        screen.contains("orphan-branch"),
        "orphan-branch should appear when [list] branches = true in config.\nScreen:\n{}",
        screen
    );
}

#[rstest]
fn test_switch_picker_create_worktree_with_alt_c(mut repo: TestRepo) {
    repo.remove_fixture_worktrees();
    // Remove origin so there's no interference from remote branches
    repo.run_git(&["remote", "remove", "origin"]);

    let env_vars = repo.test_env_vars();

    // Type branch name "new-feature", then press Alt-C (escape + c) to create
    let result = exec_in_pty_with_input_expectations(
        wt_bin().to_str().unwrap(),
        &["switch"],
        repo.root_path(),
        &env_vars,
        &[
            ("new-feature", None), // Type the branch name
            ("\x1bc", None),       // Alt-C (escape + c) to create worktree
        ],
    );

    // Alt-C triggers accept which should exit normally
    assert_eq!(
        result.exit_code, 0,
        "Expected exit code 0 for successful create"
    );

    let screen = result.screen();

    // Verify the success message shows the new branch
    assert!(
        screen.contains("new-feature") || screen.contains("Switched"),
        "Expected success message showing new-feature branch.\nScreen:\n{}",
        screen
    );

    // Verify the worktree was actually created by checking the branch exists
    let branch_output = repo
        .git_command()
        .args(["branch", "--list", "new-feature"])
        .run()
        .unwrap();
    assert!(
        String::from_utf8_lossy(&branch_output.stdout).contains("new-feature"),
        "Branch new-feature should have been created"
    );
}

#[rstest]
fn test_switch_picker_create_with_empty_query_fails(mut repo: TestRepo) {
    repo.remove_fixture_worktrees();
    // Remove origin so there's no interference from remote branches
    repo.run_git(&["remote", "remove", "origin"]);

    let env_vars = repo.test_env_vars();

    // Press Alt-C without typing a query - should error
    let result = exec_in_pty_with_input(
        wt_bin().to_str().unwrap(),
        &["switch"],
        repo.root_path(),
        &env_vars,
        "\x1bc", // Alt-C (escape + c) without typing a branch name
    );

    // Should exit with error (non-zero)
    assert_ne!(
        result.exit_code, 0,
        "Expected non-zero exit for empty query"
    );

    let screen = result.screen();

    // Verify the error message
    assert!(
        screen.contains("no branch name entered") || screen.contains("Cannot create"),
        "Expected error message about missing branch name.\nScreen:\n{}",
        screen
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
fn test_switch_picker_create_validates_templates_before_worktree(mut repo: TestRepo) {
    repo.remove_fixture_worktrees();
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
    assert_eq!(
        result.exit_code,
        0,
        "Re-running picker-create with fixed template should succeed.\nScreen:\n{}",
        result.screen()
    );

    let branch_output = repo
        .git_command()
        .args(["branch", "--list", "new-feature"])
        .run()
        .unwrap();
    assert!(
        String::from_utf8_lossy(&branch_output.stdout).contains("new-feature"),
        "Branch `new-feature` should exist after fix"
    );
}

/// Helper to create temporary directive files for PTY tests.
/// Returns (cd_path, exec_path, guards) — guards keep the temp files alive.
fn directive_files_for_pty() -> (PathBuf, PathBuf, (tempfile::TempPath, tempfile::TempPath)) {
    let cd = tempfile::NamedTempFile::new().expect("failed to create cd temp file");
    let exec = tempfile::NamedTempFile::new().expect("failed to create exec temp file");
    let cd_path = cd.path().to_path_buf();
    let exec_path = exec.path().to_path_buf();
    (
        cd_path,
        exec_path,
        (cd.into_temp_path(), exec.into_temp_path()),
    )
}

#[rstest]
fn test_switch_picker_emits_cd_directive_by_default(mut repo: TestRepo) {
    repo.remove_fixture_worktrees();
    repo.run_git(&["remote", "remove", "origin"]);

    // Create a worktree to switch to
    repo.add_worktree("target-branch");

    let (cd_path, exec_path, _guard) = directive_files_for_pty();

    let mut env_vars = repo.test_env_vars();
    env_vars.push((
        "WORKTRUNK_DIRECTIVE_CD_FILE".to_string(),
        cd_path.display().to_string(),
    ));
    env_vars.push((
        "WORKTRUNK_DIRECTIVE_EXEC_FILE".to_string(),
        exec_path.display().to_string(),
    ));

    // Run `wt switch` (without --no-cd), select "target-branch" via picker
    let result = exec_in_pty_with_input_expectations(
        wt_bin().to_str().unwrap(),
        &["switch"],
        repo.root_path(),
        &env_vars,
        &[
            // Gate on the preview-pane text that's emitted only once skim's
            // selection has moved to target-branch. Under heavy macOS load
            // skim's matcher (and the row redraw it drives) can lag the typed
            // query, but the preview pane tracks the selection cursor — and
            // Enter acts on the cursor, not on which rows are painted. Gating
            // on the preview text rides that lag instead of racing it
            // (#2334/#2729/#2767).
            ("target", Some("target-branch has no uncommitted changes")),
            ("\r", None), // Enter to switch
        ],
    );

    assert_eq!(
        result.exit_code, 0,
        "Expected exit code 0 for successful switch"
    );

    // Verify CD file DOES contain a path (default behavior)
    let cd_content = std::fs::read_to_string(&cd_path).unwrap_or_default();
    assert!(
        !cd_content.trim().is_empty(),
        "CD file should contain a path without --no-cd, got: {}",
        cd_content
    );
}

#[rstest]
fn test_switch_picker_no_cd_switches_without_cd_directive(mut repo: TestRepo) {
    repo.remove_fixture_worktrees();
    repo.run_git(&["remote", "remove", "origin"]);

    // Create a worktree to switch to
    repo.add_worktree("target-branch");

    let (cd_path, exec_path, _guard) = directive_files_for_pty();

    let mut env_vars = repo.test_env_vars();
    env_vars.push((
        "WORKTRUNK_DIRECTIVE_CD_FILE".to_string(),
        cd_path.display().to_string(),
    ));
    env_vars.push((
        "WORKTRUNK_DIRECTIVE_EXEC_FILE".to_string(),
        exec_path.display().to_string(),
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
            // Preview-pane gate: see test_switch_picker_emits_cd_directive_by_default
            // for the rationale (matcher-driven row redraw can lag the cursor).
            ("target", Some("target-branch has no uncommitted changes")),
            ("\r", None), // Enter to switch
        ],
    );

    assert_eq!(
        result.exit_code, 0,
        "Expected exit code 0 for --no-cd switch"
    );

    // The structured result reaches stdout only after execute_switch — the
    // old print-only path emitted a bare branch name and never reached it.
    let screen = result.screen();
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
    repo.remove_fixture_worktrees();
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
            // Preview-pane gate: see test_switch_picker_emits_cd_directive_by_default.
            ("target", Some("target-branch has no uncommitted changes")),
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

/// Drive the picker to remove the first non-current worktree with alt-x, then
/// switch — capturing the screen between steps so the assertion doesn't depend on
/// the picker's commit-recency row order. Returns `(landing_branch, final_screen,
/// exit_code)`, where `landing_branch` is the worktree that slid into the removed
/// row's slot (the row the sticky cursor must land on).
fn drive_alt_x_then_switch(
    args: &[&str],
    working_dir: &Path,
    env_vars: &[(String, String)],
    candidates: [&str; 2],
) -> (String, String, i32) {
    let pair = crate::common::open_pty_with_size(TERM_ROWS, TERM_COLS);

    let mut cmd = CommandBuilder::new(wt_bin().to_str().unwrap());
    for arg in args {
        cmd.arg(arg);
    }
    cmd.cwd(working_dir);
    crate::common::configure_pty_command(&mut cmd);
    cmd.env("CLICOLOR_FORCE", "1");
    cmd.env("TERM", "xterm-256color");
    for (key, value) in env_vars {
        cmd.env(key, value);
    }

    let mut child = pair.slave.spawn_command(cmd).unwrap();
    drop(pair.slave);

    let reader = pair.master.try_clone_reader().unwrap();
    let writer: crate::common::pty::SharedPtyWriter =
        Arc::new(Mutex::new(pair.master.take_writer().unwrap()));
    let rx = crate::common::pty::spawn_pty_reader_answering_queries(reader, Arc::clone(&writer));

    let mut parser = vt100::Parser::new(TERM_ROWS, TERM_COLS, 0);
    let drain = |rx: &mpsc::Receiver<Vec<u8>>, parser: &mut vt100::Parser| {
        while let Ok(chunk) = rx.try_recv() {
            parser.process(&chunk);
        }
    };
    let send = |writer: &crate::common::pty::SharedPtyWriter, bytes: &[u8]| {
        let mut w = writer.lock().unwrap();
        w.write_all(bytes).unwrap();
        w.flush().unwrap();
    };

    // Wait for skim to be ready.
    let start = Instant::now();
    loop {
        drain(&rx, &mut parser);
        if is_skim_ready(&parser.screen().contents()) || start.elapsed() > READY_TIMEOUT {
            break;
        }
        std::thread::sleep(POLL_INTERVAL);
    }

    // Both worktree rows land in one skeleton batch, so waiting for the first
    // candidate implies the second is present too.
    wait_for_stable_with_content(&rx, &mut parser, Some(candidates[0]));

    // Learn the row order: the candidate on the earlier list line is row 1 — the
    // one Down lands on and alt-x removes; the other is the sticky landing row.
    let (row1, row2) = {
        let screen = parser.screen();
        let list = screen
            .rows(0, LIST_WIDTH)
            .map(|row| row.trim_end().to_string())
            .collect::<Vec<_>>()
            .join("\n");
        let line_of = |name: &str| list.lines().position(|l| l.contains(name));
        let a = line_of(candidates[0]).expect("candidate 0 in list");
        let b = line_of(candidates[1]).expect("candidate 1 in list");
        if a < b {
            (candidates[0], candidates[1])
        } else {
            (candidates[1], candidates[0])
        }
    };

    // Down onto row 1, confirmed via the list-pane cursor pointer (cursor
    // navigation never invokes the matcher, and the pointer refreshes on every
    // render, so this is deterministic regardless of load).
    send(&writer, b"\x1b[B");
    wait_for_cursor_on_row(&rx, &mut parser, row1);

    // alt-x removes row 1; the cursor must stick to its slot — now holding row 2.
    // The pointer landing on row 2 *is* the sticky assertion: a cursor reset to
    // the top would leave the pointer on the current worktree and time this out.
    // We gate on the pointer, not row 2's preview pane, because the reposition is
    // a `Custom` action and skim doesn't repaint the preview after one — see
    // `wait_for_cursor_on_row`.
    send(&writer, b"\x1bx");
    wait_for_cursor_on_row(&rx, &mut parser, row2);

    // Enter switches to the cursor row (row 2).
    send(&writer, b"\r");
    drop(writer);

    // Let the switch finish and the process exit on its own; the kill is a
    // hung-child backstop. The wait is generous because a slow-but-successful
    // exit that gets killed reports exit 1 on Windows — see `CHILD_EXIT_TIMEOUT`.
    let start = Instant::now();
    while start.elapsed() < CHILD_EXIT_TIMEOUT {
        if child.try_wait().unwrap().is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let _ = child.kill();
    drain(&rx, &mut parser);
    let exit_code = child.wait().unwrap().exit_code() as i32;

    (row2.to_string(), parser.screen().contents(), exit_code)
}

/// alt-x keeps the cursor on the removed row's slot. After removing the first
/// non-current worktree, the cursor stays on the row that slides up (the next
/// worktree), so Enter switches there — not back to the current worktree at the
/// top, which is where skim's reload would otherwise reset the cursor.
#[rstest]
fn test_switch_picker_alt_x_keeps_cursor_sticky(mut repo: TestRepo) {
    repo.remove_fixture_worktrees();
    repo.run_git(&["remote", "remove", "origin"]);
    repo.add_worktree("wt-keep");
    repo.add_worktree("wt-drop");

    let env_vars = repo.test_env_vars();
    let (landing, screen, exit_code) = drive_alt_x_then_switch(
        &["switch", "--no-cd", "--format=json"],
        repo.root_path(),
        &env_vars,
        ["wt-keep", "wt-drop"],
    );

    // The cursor-stickiness belief is proven by the emitted result on screen,
    // not by the process exit code: the `--format=json` payload reaches stdout
    // only after the switch pipeline ran, and it names the sticky landing row
    // (not the current worktree at the top, which is where a cursor reset would
    // have switched instead).
    assert!(
        screen.contains("\"action\""),
        "switch emitted its --format=json result.\nScreen:\n{screen}"
    );
    assert!(
        screen.contains(&landing),
        "switch targeted the sticky row `{landing}`.\nScreen:\n{screen}"
    );
    // The interactive skim session's *process* exit code is a separate, weaker
    // signal than the emitted result above. It has been observed as 1 on Windows
    // even when the switch verifiably succeeded (the result lines are present) —
    // a non-zero exit from somewhere after the success output, distinct from the
    // cursor stickiness this test covers. Accept skim's selection/abort codes
    // (0 or 1) rather than pinning to 0 and flaking on that race; the same
    // tolerance `assert_valid_abort_exit_code` already applies across this
    // file's PTY tests. A gross failure (panic, crash) still fails the screen
    // assertions above, which require the success output to be present.
    assert!(
        exit_code == 0 || exit_code == 1,
        "switch after alt-x exited with an unexpected code {exit_code} (expected 0 or 1).\nScreen:\n{screen}"
    );
}

/// alt-x lands the cursor on the row that slides up — the *immediate* next row —
/// not one past it, even when the removed row has several rows below it.
///
/// [`test_switch_picker_alt_x_keeps_cursor_sticky`] removes the row directly below
/// the pinned current worktree, leaving a single row beneath it. That can't tell a
/// correct landing from a one-row overshoot: `scroll_by` clamps the cursor to the
/// list's last row, so an off-by-one lands on the same (only) remaining row and the
/// test passes anyway. This removes a *middle* row with two rows below it, where an
/// overshoot lands one row too far instead of being clamped — the exact "jumps two
/// down" a user sees with a long worktree list.
#[rstest]
fn test_switch_picker_alt_x_lands_on_immediate_next_row(mut repo: TestRepo) {
    repo.remove_fixture_worktrees();
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
    repo.remove_fixture_worktrees();
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
        Some(&format!("{remove_target} has no uncommitted changes")),
    );

    // alt-x drops the last row; the cursor lands on the new last row and its preview
    // must refresh (the removed row's preview must not linger).
    send(b"\x1bx");
    wait_for_cursor_on_row(&rx, &mut parser, &new_last);
    wait_for_stable_with_content(
        &rx,
        &mut parser,
        Some(&format!("{new_last} has no uncommitted changes")),
    );

    let preview = preview_pane_text(parser.screen());
    assert!(
        preview.contains(&format!("{new_last} has no uncommitted changes")),
        "the preview refreshed to the new last row `{new_last}`.\nPreview:\n{preview}"
    );
    assert!(
        !preview.contains(&format!("{remove_target} has no uncommitted changes")),
        "the preview must not keep showing the removed row `{remove_target}`.\nPreview:\n{preview}"
    );

    let _ = abort_and_exit_code(child, writer, rx);
}

/// alt-r refreshes the preview pane, not just the row list. The in-memory preview
/// cache is keyed by `(branch, mode)` with no SHA, so a warm working-tree diff
/// would otherwise survive an edit and re-serve stale content; the refresh clears
/// it so the pane recomputes against the current tree. Targets the pinned current
/// worktree (the top row), so the cursor sits on it before and after the reload
/// regardless of skim's reload cursor behavior — no navigation needed.
#[rstest]
fn test_switch_picker_alt_r_refreshes_preview(mut repo: TestRepo) {
    repo.remove_fixture_worktrees();
    repo.run_git(&["remote", "remove", "origin"]);

    // A committed, tracked file in the current worktree so `git diff HEAD` has
    // something to show once it's edited (untracked files don't appear in it).
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

    // The picker opens on the working-tree tab with the cursor on the pinned
    // current worktree (`main`), whose tree is clean.
    wait_for_stable_with_content(&rx, &mut parser, Some("has no uncommitted changes"));

    // Edit the tracked file *while the picker is open*. Without the refresh clearing
    // the warm cache, alt-r would re-serve the cached "no uncommitted changes" pane.
    std::fs::write(&tracked, "original\nedited\n").unwrap();

    send(b"\x1br"); // alt-r: refresh
    wait_for_stable_with_content(&rx, &mut parser, Some("diff --git"));

    let preview = preview_pane_text(parser.screen());
    assert!(
        preview.contains("diff --git"),
        "alt-r must recompute the working-tree preview to show the new edit.\nPreview:\n{preview}"
    );
    assert!(
        !preview.contains("has no uncommitted changes"),
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
    repo.remove_fixture_worktrees();
    repo.run_git(&["remote", "remove", "origin"]);
    repo.add_worktree("solo-wt");

    let env_vars = repo.test_env_vars();
    let result = exec_in_pty_capture_before_abort(
        wt_bin().to_str().unwrap(),
        &["switch"],
        repo.root_path(),
        &env_vars,
        &[
            ("solo-wt", Some("solo-wt")), // filter to the sole matching worktree
            ("\x1bx", None),              // alt-x removes it; query now matches nothing
            ("\x1bx", None),              // alt-x again with nothing selected: a no-op
        ],
    );

    assert_valid_abort_exit_code(result.exit_code);
}

/// alt-x on an unmerged branch-only row leaves the row present end-to-end through
/// real skim — `SafeDelete` refuses to delete an unmerged branch, so the row must
/// not vanish. The inverse of `…_no_match_stays_responsive`, which removes a row
/// and expects emptiness.
///
/// This guards the integration outcome (the real reload re-renders the row, the
/// list doesn't empty), not the no-flicker mechanism specifically: a regression in
/// the up-front keep would be masked here by the background restore backstop, which
/// re-inserts the row. The keep-path-specific "never dropped, no flicker" property
/// is unit-tested in `test_invoke_keeps_unmerged_branch_only_row`, which checks the
/// synchronous `items` state before any backstop can run.
#[rstest]
fn test_switch_picker_alt_x_keeps_unmerged_branch_row(mut repo: TestRepo) {
    repo.remove_fixture_worktrees();
    repo.run_git(&["remote", "remove", "origin"]);

    // An unmerged branch — a commit off the default branch — with no worktree.
    // Use the branch wt itself resolves as the default, so the picker's
    // integration check and this checkout key off the same ref.
    let default_branch = repo.git_output(&["symbolic-ref", "--short", "HEAD"]);
    repo.run_git(&["checkout", "-b", "unmerged-orphan"]);
    std::fs::write(repo.root_path().join("orphan.txt"), "unmerged work").unwrap();
    repo.run_git(&["add", "."]);
    repo.run_git(&["commit", "-m", "unmerged work"]);
    repo.run_git(&["checkout", &default_branch]);

    let env_vars = repo.test_env_vars();
    let PickerSession {
        child,
        _master,
        writer,
        rx,
        mut parser,
    } = boot_picker_pty(
        wt_bin().to_str().unwrap(),
        &["switch", "--branches"],
        repo.root_path(),
        &env_vars,
    );

    // Filter to the branch-only row, then wait for the cursor (`>`) to land on it.
    // A local branch with no worktree carries the `/` gutter, so `/ unmerged-orphan`
    // names the data row specifically — skim's query-echo prompt line is
    // `> unmerged-orphan` (no gutter), which this gate ignores. Keying off the
    // gutter rather than the bare name is what makes the wait robust: under Windows
    // CI load the prompt echo trails its keystrokes (the final character can still
    // be unrendered once the row is already on screen), and an assertion that
    // counted the name across both the prompt and the row flaked when the prompt
    // came up a character short.
    send_input_awaiting_content(&writer, &rx, &mut parser, "unmerged-orphan", None);
    wait_for_cursor_on_row(&rx, &mut parser, "/ unmerged-orphan");

    // alt-x: `SafeDelete` refuses to drop an unmerged branch, so the row stays and
    // the cursor holds on `/ unmerged-orphan`. A regression that dropped the row
    // would empty the filtered list, the `/` data row would never reappear, and
    // this wait would time out with a screen dump.
    send_input_awaiting_content(&writer, &rx, &mut parser, "\x1bx", None);
    wait_for_cursor_on_row(&rx, &mut parser, "/ unmerged-orphan");

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
    repo.remove_fixture_worktrees();
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
    let pair = crate::common::open_pty_with_size(TERM_ROWS, TERM_COLS);
    let mut cmd = CommandBuilder::new(wt_bin().to_str().unwrap());
    cmd.arg("switch");
    cmd.cwd(repo.root_path());
    crate::common::configure_pty_command(&mut cmd);
    cmd.env("CLICOLOR_FORCE", "1");
    cmd.env("TERM", "xterm-256color");
    for (key, value) in &env_vars {
        cmd.env(key, value);
    }
    let mut child = pair.slave.spawn_command(cmd).unwrap();
    drop(pair.slave);

    let reader = pair.master.try_clone_reader().unwrap();
    let writer: crate::common::pty::SharedPtyWriter =
        Arc::new(Mutex::new(pair.master.take_writer().unwrap()));
    let rx = crate::common::pty::spawn_pty_reader_answering_queries(reader, Arc::clone(&writer));
    let mut parser = vt100::Parser::new(TERM_ROWS, TERM_COLS, 0);
    let drain = |rx: &mpsc::Receiver<Vec<u8>>, parser: &mut vt100::Parser| {
        while let Ok(chunk) = rx.try_recv() {
            parser.process(&chunk);
        }
    };
    let send = |writer: &crate::common::pty::SharedPtyWriter, bytes: &[u8]| {
        let mut w = writer.lock().unwrap();
        w.write_all(bytes).unwrap();
        w.flush().unwrap();
    };

    // Wait for skim to be ready, then for the worktree row to land.
    let start = Instant::now();
    loop {
        drain(&rx, &mut parser);
        if is_skim_ready(&parser.screen().contents()) || start.elapsed() > READY_TIMEOUT {
            break;
        }
        std::thread::sleep(POLL_INTERVAL);
    }
    wait_for_stable_with_content(&rx, &mut parser, Some("transform-me"));

    // Filter to the single worktree row so the selection is deterministic
    // regardless of commit-recency order, then confirm the cursor is on it. The
    // row still reads `> + transform-me` — a linked worktree.
    send(&writer, b"transform-me");
    wait_for_cursor_on_row(&rx, &mut parser, "+ transform-me");

    // alt-x morphs the row in place. The cursor must land back on the morphed
    // row — `> / transform-me`, gutter flipped to the branch sigil. The morph
    // leaves `search_text` untouched, so the row still matches the active filter;
    // a drop would empty the list, and the old re-collect would reset the cursor.
    send(&writer, b"\x1bx");
    wait_for_cursor_on_row(&rx, &mut parser, "/ transform-me");

    send(&writer, b"\x1b"); // Esc to exit the picker.
    drop(writer);
    let start = Instant::now();
    while start.elapsed() < CHILD_EXIT_TIMEOUT {
        if child.try_wait().unwrap().is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let _ = child.kill();
    drain(&rx, &mut parser);
    let _ = child.wait();

    // The worktree is gone but its branch survives — the morph's premise.
    assert!(
        !wt_path.exists(),
        "alt-x removed the worktree directory.\nScreen:\n{}",
        parser.screen().contents()
    );
    let branches = repo.git_output(&["branch", "--list", "transform-me"]);
    assert!(
        branches.contains("transform-me"),
        "the unmerged branch is retained after its worktree is removed: {branches:?}"
    );
}

/// alt-x on the *current* worktree — the one the picker was launched from — keeps
/// the row in place rather than removing it. Removing the worktree the shell is
/// sitting in would have to cd elsewhere first, which drags `post-switch` hooks
/// into the picker and swaps an empty placeholder under the cursor mid-render, so
/// the picker declines (`removal_targets_current_worktree`). End-to-end through
/// real skim launched from inside the worktree: after alt-x the cursor stays on
/// the unchanged `@ standing-here` row (gutter still `@` — not dropped, not
/// morphed to `/`) and the worktree survives on disk. The branch is unmerged, so
/// were the guard absent the row would morph; a surviving `@` proves the
/// current-worktree check fires before the morph path.
#[rstest]
fn test_switch_picker_alt_x_keeps_current_worktree(mut repo: TestRepo) {
    repo.remove_fixture_worktrees();
    repo.run_git(&["remote", "remove", "origin"]);

    let wt_path = repo.add_worktree("standing-here");
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
    let pair = crate::common::open_pty_with_size(TERM_ROWS, TERM_COLS);
    let mut cmd = CommandBuilder::new(wt_bin().to_str().unwrap());
    cmd.arg("switch");
    cmd.cwd(&wt_path); // launched from inside the worktree → it's the current one
    crate::common::configure_pty_command(&mut cmd);
    cmd.env("CLICOLOR_FORCE", "1");
    cmd.env("TERM", "xterm-256color");
    for (key, value) in &env_vars {
        cmd.env(key, value);
    }
    let mut child = pair.slave.spawn_command(cmd).unwrap();
    drop(pair.slave);

    let reader = pair.master.try_clone_reader().unwrap();
    let writer: crate::common::pty::SharedPtyWriter =
        Arc::new(Mutex::new(pair.master.take_writer().unwrap()));
    let rx = crate::common::pty::spawn_pty_reader_answering_queries(reader, Arc::clone(&writer));
    let mut parser = vt100::Parser::new(TERM_ROWS, TERM_COLS, 0);
    let drain = |rx: &mpsc::Receiver<Vec<u8>>, parser: &mut vt100::Parser| {
        while let Ok(chunk) = rx.try_recv() {
            parser.process(&chunk);
        }
    };
    let send = |writer: &crate::common::pty::SharedPtyWriter, bytes: &[u8]| {
        let mut w = writer.lock().unwrap();
        w.write_all(bytes).unwrap();
        w.flush().unwrap();
    };

    let start = Instant::now();
    loop {
        drain(&rx, &mut parser);
        if is_skim_ready(&parser.screen().contents()) || start.elapsed() > READY_TIMEOUT {
            break;
        }
        std::thread::sleep(POLL_INTERVAL);
    }
    wait_for_stable_with_content(&rx, &mut parser, Some("standing-here"));

    // Filter to the current-worktree row so the selection is deterministic, then
    // confirm the cursor is on it — `> @ standing-here`, the `@` current gutter.
    send(&writer, b"standing-here");
    wait_for_cursor_on_row(&rx, &mut parser, "@ standing-here");

    // alt-x is declined for the current worktree: the row neither drops (the
    // filtered list would empty) nor morphs (the gutter would flip to `/`). After
    // the reload settles the cursor is back on the unchanged `@ standing-here` row.
    send(&writer, b"\x1bx");
    wait_for_cursor_on_row(&rx, &mut parser, "@ standing-here");

    send(&writer, b"\x1b"); // Esc to exit the picker.
    drop(writer);
    let start = Instant::now();
    while start.elapsed() < CHILD_EXIT_TIMEOUT {
        if child.try_wait().unwrap().is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let _ = child.kill();
    drain(&rx, &mut parser);
    let _ = child.wait();

    // The worktree survives — alt-x declined to remove the one we're standing in.
    assert!(
        wt_path.exists(),
        "alt-x removed the current worktree.\nScreen:\n{}",
        parser.screen().contents()
    );
}

/// alt-x on an unremovable row (here the main worktree) keeps the cursor exactly on
/// that row — it must not drift down — even with several rows below it.
///
/// This mirrors the keep paths a user hits most: alt-x on the main worktree or a
/// dirty worktree surfaces a diagnostic and keeps the row. The single-row filtered
/// case in [`test_switch_picker_alt_x_keeps_current_worktree`] can't catch a
/// one-row drift (only one row matches the filter), so this drives a full,
/// unfiltered list and removes nothing: the cursor has to come back to the *same*
/// row, not the one below it.
#[rstest]
fn test_switch_picker_alt_x_unremovable_row_keeps_cursor(mut repo: TestRepo) {
    repo.remove_fixture_worktrees();
    repo.run_git(&["remote", "remove", "origin"]);
    // Launch from `wt-a` so it's the pinned current row; `main` then sorts second,
    // with `wt-b`/`wt-c`/`wt-d` below it — main is a mid-list unremovable row.
    let wt_a = repo.add_worktree("wt-a");
    for branch in ["wt-b", "wt-c", "wt-d"] {
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
        &wt_a,
        &env_vars,
    );
    let send = |bytes: &[u8]| {
        let mut w = writer.lock().unwrap();
        w.write_all(bytes).unwrap();
        w.flush().unwrap();
    };

    wait_for_stable_with_content(&rx, &mut parser, Some("wt-d"));

    // Down from the pinned current `wt-a` onto the `main` row (sorted second).
    send(b"\x1b[B");
    wait_for_cursor_on_row(&rx, &mut parser, "main");

    // alt-x is declined for the main worktree (it can't be removed). The row stays
    // and the cursor must land back on it — a one-row drift lands on the worktree
    // below `main` and times this out.
    send(b"\x1bx");
    wait_for_cursor_on_row(&rx, &mut parser, "main");

    let _ = abort_and_exit_code(child, writer, rx);
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
    repo.remove_fixture_worktrees();
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
