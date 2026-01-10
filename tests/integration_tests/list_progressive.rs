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

    // Verify we captured output quickly (within reasonable time)
    assert!(
        output.total_duration.as_secs() < 5,
        "Command should complete in under 5 seconds, took {:?}",
        output.total_duration
    );

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
}
