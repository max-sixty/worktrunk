//! Tests for progressive rendering in `wt list`
//!
//! These tests capture multiple snapshots of the output as it renders,
//! verifying that the table structure appears first and data fills in progressively.
#![cfg(all(unix, feature = "shell-integration-tests"))]

use crate::common::progressive_output::{ProgressiveCaptureOptions, capture_progressive_output};
use crate::common::{TestRepo, repo};
use rstest::rstest;

/// Tests progressive rendering with multiple worktrees.
/// Verifies: headers appear immediately, dots decrease over time, all worktrees visible.
/// (Consolidates previous tests: rendering_basic, dots_decrease, many_worktrees)
#[rstest]
fn test_list_progressive_rendering(mut repo: TestRepo) {
    // Create many worktrees to ensure rendering takes time
    for i in 1..=10 {
        repo.add_worktree(&format!("branch-{:02}", i));
    }

    let output = capture_progressive_output(
        &repo,
        "list",
        &["--full", "--branches"],
        ProgressiveCaptureOptions::with_byte_interval(500),
    );

    // Basic assertions
    assert_eq!(output.exit_code, 0);
    assert!(
        output.stages.len() >= 3,
        "Should capture at least 3 stages with many worktrees, got {}",
        output.stages.len()
    );

    // Verify progressive filling: dots should decrease over time
    output.verify_progressive_filling().unwrap();

    // Verify table header appears in initial output
    let initial = output.initial().visible_text();
    assert!(
        initial.contains("Branch"),
        "Table header should appear immediately"
    );
    assert!(
        initial.contains("Status"),
        "Status column header should appear immediately"
    );

    // Verify final output has all worktrees
    let final_output = output.final_output();
    for i in 1..=10 {
        assert!(
            final_output.contains(&format!("branch-{:02}", i)),
            "Final output should contain branch-{:02}",
            i
        );
    }
}

/// Tests progressive output capture API: timestamps and snapshot_at.
/// (Consolidates previous tests: timing, snapshot_at)
#[rstest]
fn test_list_progressive_api(mut repo: TestRepo) {
    repo.add_worktree("feature");

    let output = capture_progressive_output(
        &repo,
        "list",
        &[],
        ProgressiveCaptureOptions::with_byte_interval(600),
    );

    // Verify timestamps are monotonically increasing
    for i in 1..output.stages.len() {
        assert!(
            output.stages[i].timestamp >= output.stages[i - 1].timestamp,
            "Timestamps should increase monotonically"
        );
    }

    // Test snapshot_at API
    let snapshot = output.snapshot_at(std::time::Duration::from_millis(100));
    assert!(
        !snapshot.visible_text().is_empty(),
        "Snapshot should have content"
    );
    assert!(
        snapshot.timestamp < output.total_duration,
        "Snapshot should be before end"
    );
}

/// Tests overflow mode: when worktrees exceed terminal height, the skeleton shows a subset,
/// then finalize erases and prints the complete table (scrolls naturally).
#[rstest]
fn test_list_progressive_overflow(mut repo: TestRepo) {
    // Create enough worktrees to overflow a 10-row terminal.
    // With height=10: visible_rows = 10 - 4 (header + spacer + footer + cursor)
    // - 2 (prompt reserve) = 4. 10 worktrees + main = 11 rows, well above that.
    for i in 1..=10 {
        repo.add_worktree(&format!("overflow-{:02}", i));
    }

    let mut opts = ProgressiveCaptureOptions::with_byte_interval(500);
    opts.terminal_size = (10, 150); // Short terminal triggers overflow

    let output = capture_progressive_output(&repo, "list", &["--full", "--branches"], opts);

    assert_eq!(output.exit_code, 0);

    // The overflow finalize path erases the skeleton and prints the complete table,
    // which scrolls naturally. The vt100 parser (10 rows, no scrollback) only captures
    // the visible tail. Verify: the footer and later branches are visible, confirming
    // the overflow finalize path executed and printed the full table.
    let final_text = output.final_output();

    // Footer should be visible at the bottom
    assert!(
        final_text.contains("Showing"),
        "Footer should be visible after overflow finalize.\nFinal output:\n{final_text}"
    );

    // Later branches should be visible (earlier ones scrolled off the 10-row viewport)
    assert!(
        final_text.contains("overflow-10"),
        "Last branch should be visible.\nFinal output:\n{final_text}"
    );

    // No placeholder dots should remain — finalize printed real data
    assert!(
        !final_text.contains('·'),
        "No placeholder dots should remain after finalize.\nFinal output:\n{final_text}"
    );

    // The prompt reserve leaves the cursor two rows above the bottom of the
    // scrolled screen, with the footer directly above it — the shell prompt
    // printed after exit renders into the reserved rows instead of scrolling
    // the settled table.
    assert_eq!(
        output.final_cursor,
        (7, 0),
        "Cursor should rest 2 reserved rows above the bottom of the 10-row \
         terminal.\nFinal output:\n{final_text}"
    );
}

/// Tests progressive rendering with no worktrees (fast path).
#[rstest]
fn test_list_progressive_fast_command(repo: TestRepo) {
    let output = capture_progressive_output(
        &repo,
        "list",
        &[],
        ProgressiveCaptureOptions::with_byte_interval(600),
    );

    assert_eq!(output.exit_code, 0);

    // Even fast commands should capture at least the final state
    assert!(
        !output.stages.is_empty(),
        "Should capture at least one snapshot"
    );

    assert!(
        output.final_output().contains("Branch"),
        "Should have table header"
    );

    // The prompt-reserve rows are emitted below the footer with the cursor
    // moved back over them: it rests directly under the last content row, so
    // a reserve/MoveUp mismatch would show up as a displaced cursor.
    let content_rows = output.final_output().trim_end().lines().count() as u16;
    assert_eq!(
        output.final_cursor,
        (content_rows, 0),
        "Cursor should rest directly under the footer.\nFinal output:\n{}",
        output.final_output()
    );
}
