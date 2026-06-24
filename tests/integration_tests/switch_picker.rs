#![cfg(all(unix, feature = "shell-integration-tests"))]
//! TUI snapshot tests for `wt switch` interactive picker
//!
//! These tests use PTY execution combined with vt100 terminal emulation to capture
//! what the user actually sees on screen, enabling meaningful snapshot testing of
//! the skim-based TUI interface.
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
use crate::common::{TestRepo, repo, wt_bin};
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

/// How long screen must be unchanged to consider it "stable".
/// Must be long enough for preview content to load (preview commands run async).
/// 500ms balances reliability (allows preview to complete) with speed.
/// Panel switches trigger async git commands that may take time.
const STABLE_DURATION: Duration = Duration::from_millis(500);

/// Polling interval when waiting for output.
/// Fast polling ensures tests complete quickly when ready.
const POLL_INTERVAL: Duration = Duration::from_millis(10);

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
        let list = screen
            .rows(0, LIST_WIDTH)
            .map(|row| row.trim_end().to_string())
            .collect::<Vec<_>>()
            .join("\n")
            .trim_end()
            .to_string();
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
    let pair = crate::common::open_pty_with_size(TERM_ROWS, TERM_COLS);

    let mut cmd = CommandBuilder::new(command);
    for arg in args {
        cmd.arg(arg);
    }
    cmd.cwd(working_dir);

    // Set up isolated environment with coverage passthrough
    crate::common::configure_pty_command(&mut cmd);
    cmd.env("CLICOLOR_FORCE", "1");
    cmd.env("TERM", "xterm-256color");

    // Add test-specific environment variables
    for (key, value) in env_vars {
        cmd.env(key, value);
    }

    let mut child = pair.slave.spawn_command(cmd).unwrap();
    drop(pair.slave);

    let reader = pair.master.try_clone_reader().unwrap();
    let writer: crate::common::pty::SharedPtyWriter =
        Arc::new(Mutex::new(pair.master.take_writer().unwrap()));

    // Drain PTY output into a channel; the reader thread also answers skim's
    // startup cursor-position query (see `spawn_pty_reader_answering_queries`).
    let rx = crate::common::pty::spawn_pty_reader_answering_queries(reader, Arc::clone(&writer));

    let mut parser = vt100::Parser::new(TERM_ROWS, TERM_COLS, 0);

    // Helper to drain available output from the channel (non-blocking)
    let drain_output = |rx: &mpsc::Receiver<Vec<u8>>, parser: &mut vt100::Parser| {
        while let Ok(chunk) = rx.try_recv() {
            parser.process(&chunk);
        }
    };

    // Wait for skim to be ready (show "> " prompt)
    let start = Instant::now();
    loop {
        drain_output(&rx, &mut parser);

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

    // Send each input and wait for screen to stabilize after each
    for (input, expected_content) in inputs {
        {
            let mut w = writer.lock().unwrap();
            w.write_all(input.as_bytes()).unwrap();
            w.flush().unwrap();
        }

        // Wait for screen to stabilize after this input, optionally requiring specific content
        wait_for_stable_with_content(&rx, &mut parser, *expected_content);
    }

    // Release the main thread's writer handle. The reader thread holds the
    // other Arc clone until the PTY drains, so this no longer drives stdin EOF.
    // The picker exits on Accept/Escape, or the kill below.
    drop(writer);

    // Poll for process exit (fast polling, long timeout for CI)
    let start = std::time::Instant::now();
    let timeout = Duration::from_secs(5);
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
    let pair = crate::common::open_pty_with_size(TERM_ROWS, TERM_COLS);

    let mut cmd = CommandBuilder::new(command);
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

    let drain_output = |rx: &mpsc::Receiver<Vec<u8>>, parser: &mut vt100::Parser| {
        while let Ok(chunk) = rx.try_recv() {
            parser.process(&chunk);
        }
    };

    // Wait for skim to be ready
    let start = Instant::now();
    loop {
        drain_output(&rx, &mut parser);

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

    // Send pre-abort inputs (filter text, panel switches, etc.)
    for (input, expected_content) in pre_abort_inputs {
        {
            let mut w = writer.lock().unwrap();
            w.write_all(input.as_bytes()).unwrap();
            w.flush().unwrap();
        }
        wait_for_stable_with_content(&rx, &mut parser, *expected_content);
    }

    // === CAPTURE: screen state is now stable — snapshot BEFORE aborting ===
    // The parser retains this state because we stop feeding output to it.

    // Send Escape to abort
    {
        let mut w = writer.lock().unwrap();
        w.write_all(b"\x1b").unwrap();
        w.flush().unwrap();
    }
    drop(writer);

    // Drain remaining output WITHOUT feeding to parser — preserves pre-abort screen
    let start = Instant::now();
    let timeout = Duration::from_secs(5);
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

    let exit_status = child.wait().unwrap();
    let exit_code = exit_status.exit_code() as i32;

    PtyResult { parser, exit_code }
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
/// Handles a subtle race condition: skim may continuously redraw (cursor repositioning,
/// border repaints) even after all meaningful content is rendered. These minor redraws
/// reset the stability timer, preventing the "no changes for 500ms" condition from
/// being met. To handle this, once expected content is found, we track how long it
/// has been continuously present and accept stability after STABLE_DURATION even if
/// the screen keeps changing cosmetically.
///
/// Tip: avoid including the panel border character (`│`) in `expected_content` —
/// its rendering varies by platform and terminal, causing flaky assertions.
fn wait_for_stable_with_content(
    rx: &mpsc::Receiver<Vec<u8>>,
    parser: &mut vt100::Parser,
    expected_content: Option<&str>,
) {
    let start = Instant::now();
    let mut last_change = Instant::now();
    let mut last_content = parser.screen().contents();
    // Tracks when expected content first appeared continuously on screen.
    // Used as a fallback stability signal when skim keeps redrawing cosmetically.
    let mut content_found_at: Option<Instant> = None;

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

        // Check if expected content is present (if required)
        let content_ready = match expected_content {
            Some(expected) => {
                let found = current_content.contains(expected);
                if found {
                    content_found_at.get_or_insert(Instant::now());
                } else {
                    // Content disappeared (e.g., skim full redraw) — reset
                    content_found_at = None;
                }
                found
            }
            None => true,
        };

        // Primary: screen hasn't changed for STABLE_DURATION and content is ready
        if last_change.elapsed() >= STABLE_DURATION && content_ready {
            return;
        }

        // Fallback for content-expected case: if expected content has been continuously
        // present for STABLE_DURATION, consider the screen stable even if skim keeps
        // doing cosmetic redraws (cursor repositioning, border repaints). These minor
        // changes don't affect snapshot correctness.
        if let Some(found_time) = content_found_at
            && found_time.elapsed() >= STABLE_DURATION
        {
            return;
        }

        std::thread::sleep(POLL_INTERVAL);
    }

    // Timeout: if expected content was specified but not found, fail with diagnostics
    // instead of proceeding to a guaranteed snapshot mismatch.
    if let Some(expected) = expected_content
        && !last_content.contains(expected)
    {
        panic!(
            "Timed out after {:?} waiting for expected content {:?} to appear on screen.\n\
             Screen content:\n{}",
            STABILIZE_TIMEOUT, expected, last_content
        );
    }

    // Stability-only timeout (no content expectation, or content present but unstable) —
    // warn but proceed (test may still pass with current screen state)
    eprintln!(
        "Warning: Screen did not fully stabilize within {:?}",
        STABILIZE_TIMEOUT
    );
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
    let result = exec_in_pty_capture_before_abort(
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
            ("\x1b[B", None),             // Down: move cursor to `feature`
            ("\x1b5", Some("Configure")), // Alt-5: summary panel; wait for hint
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

    let mut env_vars = repo.test_env_vars();
    env_vars.push((
        "MOCK_CONFIG_DIR".to_string(),
        mock_bin.display().to_string(),
    ));
    let base_path = std::env::var("PATH").unwrap_or_default();
    env_vars.push((
        "PATH".to_string(),
        format!("{}:{base_path}", mock_bin.display()),
    ));
    env_vars
}

/// `wt switch --prs` on a GitHub repo: the open-PR list streams into the picker
/// via a mocked `gh pr list`. Asserts the PR row reaches the list (the `#42`
/// reference in the CI column), which deterministically exercises the whole
/// fetch → stream → render path (`fetch_open_prs`, `fetch_github`,
/// `parse_github_prs`, `stream_open_prs`, `PrSkimItem::new`, `render_grid_row`,
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
        // Wait for the PR row to stream into the list (the `#42` content gate),
        // then assert on the row itself — deterministic, with no dependency on
        // selecting an async-arrived row.
        &[("", Some("#42"))],
    );

    assert_valid_abort_exit_code(result.exit_code);
    let (list, _preview) = result.panels();
    // `#42` in the CI column; the head branch is truncated in the narrow list.
    assert!(list.contains("#42"), "PR number in list:\n{list}");
    // The title is NOT on the row — it lives in the preview so columns align.
    assert!(
        !list.contains("Retry the flaky network test"),
        "PR title should stay off the row:\n{list}"
    );
    // The header's loading marker is gone once the rows have streamed in.
    assert!(
        !list.contains("loading open PRs"),
        "loading marker cleared once rows arrived:\n{list}"
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
        // `!7` (GitLab MR ref) is the row's stable substring; the title lives in
        // the preview, not the row.
        &[("", Some("!7"))],
    );

    assert_valid_abort_exit_code(result.exit_code);
    let (list, _preview) = result.panels();
    // `!7` (GitLab MR ref) in the CI column; source branch truncates.
    assert!(list.contains("!7"), "MR number in list:\n{list}");
    assert!(
        !list.contains("Cache the dependency graph"),
        "MR title should stay off the row:\n{list}"
    );
}

/// `wt switch --prs` shows a dim "loading open PRs…" marker on the header row
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
        &[("", Some("loading open PRs"))],
    );

    assert_valid_abort_exit_code(result.exit_code);
    let (list, _preview) = result.panels();
    assert!(
        list.contains("loading open PRs"),
        "loading line on the header while --prs fetches:\n{list}"
    );
    // The PR row hasn't streamed in yet — still inside the delayed forge call.
    assert!(!list.contains("#42"), "rows not yet streamed in:\n{list}");
}

#[rstest]
fn test_switch_picker_preview_cycle_tab_forward(mut repo: TestRepo) {
    // Tab cycles the preview tab forward. From the default HEAD± tab (1), one
    // Tab lands on the log tab (2), proving the `tr 123456 234561` rotation in
    // the keybinding runs under the real shell. (alt-1..alt-6 jump directly; the
    // panel tests above cover those — this covers the cycle path.)
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
    // Tab returns to the HEAD± tab. This covers the `7 → 1` end of the
    // `tr 1234567 2345671` map, the half most easily typo'd away.
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
            // Alt-7: jump to comments (7). On a worktree row the comments tab
            // points at `--prs` rows (it's fetched only there).
            ("\x1b7", Some("--prs")),
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
    // `tr 1234567 7123456` including the 1 → 7 wraparound.
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
            // Shift-Tab: HEAD± (1) → comments (7), the 1 → 7 wraparound. The
            // comments tab on a worktree row points at `--prs` rows.
            ("\x1b[Z", Some("--prs")),
        ],
    );

    assert_valid_abort_exit_code(result.exit_code);

    let (_list, preview) = result.panels();
    assert!(
        preview.contains("Comments show on"),
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
            ("n\n", None),                  // decline
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

/// Drive the picker to remove the first non-current worktree with alt-r, then
/// switch — capturing the screen between steps so the assertion doesn't depend on
/// the picker's commit-recency row order. Returns `(landing_branch, final_screen,
/// exit_code)`, where `landing_branch` is the worktree that slid into the removed
/// row's slot (the row the sticky cursor must land on).
fn drive_alt_r_then_switch(
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
    // one Down lands on and alt-r removes; the other is the sticky landing row.
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

    // Down onto row 1, confirmed via its preview pane (cursor navigation never
    // invokes the matcher, so this is deterministic regardless of load).
    send(&writer, b"\x1b[B");
    wait_for_stable_with_content(
        &rx,
        &mut parser,
        Some(&format!("{row1} has no uncommitted changes")),
    );

    // alt-r removes row 1; the cursor must stick to its slot — now holding row 2.
    // Gating on row 2's preview *is* the sticky assertion: a cursor reset to the
    // top would show the current worktree's preview and time this out.
    send(&writer, b"\x1br");
    wait_for_stable_with_content(
        &rx,
        &mut parser,
        Some(&format!("{row2} has no uncommitted changes")),
    );

    // Enter switches to the cursor row (row 2).
    send(&writer, b"\r");
    drop(writer);

    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(5) {
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

/// alt-r keeps the cursor on the removed row's slot. After removing the first
/// non-current worktree, the cursor stays on the row that slides up (the next
/// worktree), so Enter switches there — not back to the current worktree at the
/// top, which is where skim's reload would otherwise reset the cursor.
#[rstest]
fn test_switch_picker_alt_r_keeps_cursor_sticky(mut repo: TestRepo) {
    repo.remove_fixture_worktrees();
    repo.run_git(&["remote", "remove", "origin"]);
    repo.add_worktree("wt-keep");
    repo.add_worktree("wt-drop");

    let env_vars = repo.test_env_vars();
    let (landing, screen, exit_code) = drive_alt_r_then_switch(
        &["switch", "--no-cd", "--format=json"],
        repo.root_path(),
        &env_vars,
        ["wt-keep", "wt-drop"],
    );

    assert_eq!(
        exit_code, 0,
        "switch after alt-r should exit 0.\nScreen:\n{screen}"
    );
    // The structured result reaches stdout only after the switch pipeline ran;
    // it targets the sticky row, not the current worktree.
    assert!(
        screen.contains("\"action\""),
        "switch emitted its --format=json result.\nScreen:\n{screen}"
    );
    assert!(
        screen.contains(&landing),
        "switch targeted the sticky row `{landing}`.\nScreen:\n{screen}"
    );
}

/// Removing the sole row matching an active query leaves the filtered list empty,
/// so the cursor-reposition gives up once the matcher settles empty rather than
/// spinning the event loop. The picker stays responsive — its screen stabilizes,
/// then aborts cleanly.
#[rstest]
fn test_switch_picker_alt_r_no_match_stays_responsive(mut repo: TestRepo) {
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
            ("\x1br", None),              // alt-r removes it; query now matches nothing
        ],
    );

    assert_valid_abort_exit_code(result.exit_code);
}
