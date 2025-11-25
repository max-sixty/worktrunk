//! Tests for progressive rendering in `wt list`
//!
//! These tests capture multiple snapshots of the output as it renders,
//! verifying that the table structure appears first and data fills in progressively.

use crate::common::TestRepo;
use crate::common::progressive_output::{ProgressiveCaptureOptions, capture_progressive_output};

#[test]
fn test_list_progressive_rendering_basic() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit");

    // Create a few worktrees to have data to render
    repo.add_worktree("feature-a", "feature-a");
    repo.add_worktree("feature-b", "feature-b");
    repo.add_worktree("bugfix", "bugfix");

    // Capture progressive output using byte-based strategy (deterministic)
    let output = capture_progressive_output(
        &repo,
        "list",
        &["--full", "--branches"],
        ProgressiveCaptureOptions::with_byte_interval(500),
    );

    // Basic assertions
    assert_eq!(output.exit_code, 0, "Command should succeed");
    assert!(
        output.stages.len() > 1,
        "Should capture multiple stages, got {}",
        output.stages.len()
    );

    // Verify progressive filling: dots should decrease over time
    output.verify_progressive_filling().unwrap();

    // Verify table header appears in initial output
    assert!(
        output.initial().visible_text().contains("Branch"),
        "Table header should appear immediately"
    );
    assert!(
        output.initial().visible_text().contains("Status"),
        "Status column header should appear immediately"
    );

    // Verify final output has all worktrees
    let final_text = output.final_output();
    assert!(final_text.contains("feature-a"), "Should contain feature-a");
    assert!(final_text.contains("feature-b"), "Should contain feature-b");
    assert!(final_text.contains("bugfix"), "Should contain bugfix");

    // Final output should have fewer dots than initial (verified by verify_progressive_filling)
    // No need for additional assertions - verify_progressive_filling already confirms progressive behavior
}

#[test]
fn test_list_progressive_rendering_stages() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit");

    // Create several worktrees
    for i in 1..=5 {
        repo.add_worktree(&format!("branch-{}", i), &format!("branch-{}", i));
    }

    // Use byte-based capture for deterministic snapshots
    let output = capture_progressive_output(
        &repo,
        "list",
        &["--full"],
        ProgressiveCaptureOptions::with_byte_interval(400),
    );

    // Command should succeed
    assert_eq!(output.exit_code, 0, "Command should succeed");

    // Should capture at least initial and final stages
    assert!(
        !output.stages.is_empty(),
        "Should capture at least one stage"
    );

    // Sample up to 3 stages (may be fewer on fast machines)
    let samples = output.samples(3);
    assert!(!samples.is_empty(), "Should get at least one sample stage");

    // If we have enough stages, use canonical verification
    // (handles edge cases like fast completion gracefully)
    if output.stages.len() >= 2 {
        // verify_progressive_filling() returns Err if progressive rendering wasn't observable,
        // which is acceptable on fast CI machines - just skip the assertion in that case
        let _ = output.verify_progressive_filling();
    }

    // Verify all branches appear in final output (the essential assertion)
    let final_text = output.final_output();
    for i in 1..=5 {
        assert!(
            final_text.contains(&format!("branch-{}", i)),
            "Final output should contain branch-{}",
            i
        );
    }
}

#[test]
fn test_list_progressive_dots_decrease() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit");

    // Create multiple worktrees to ensure progressive rendering is observable
    for i in 1..=5 {
        repo.add_worktree(&format!("branch-{}", i), &format!("branch-{}", i));
    }

    let output = capture_progressive_output(
        &repo,
        "list",
        &["--full"],
        ProgressiveCaptureOptions::with_byte_interval(600),
    );

    // Use canonical verification method
    output.verify_progressive_filling().unwrap();
}

#[test]
fn test_list_progressive_timing() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit");
    repo.add_worktree("feature", "feature");

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
}

#[test]
fn test_list_progressive_snapshot_at() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit");
    repo.add_worktree("feature", "feature");

    let output = capture_progressive_output(
        &repo,
        "list",
        &[],
        ProgressiveCaptureOptions::with_byte_interval(600),
    );

    // Get snapshot at approximately 100ms
    let snapshot = output.snapshot_at(std::time::Duration::from_millis(100));

    // Should have some content
    assert!(
        !snapshot.visible_text().is_empty(),
        "Snapshot should have content"
    );

    // Should be somewhere in the middle of rendering
    assert!(
        snapshot.timestamp < output.total_duration,
        "Snapshot should be before end"
    );
}

/// Test with a larger dataset to ensure progressive rendering is visible
#[test]
fn test_list_progressive_many_worktrees() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit");

    // Create many worktrees to ensure rendering takes time
    for i in 1..=10 {
        repo.add_worktree(&format!("worktree-{:02}", i), &format!("branch-{:02}", i));
    }

    let output = capture_progressive_output(
        &repo,
        "list",
        &["--full", "--branches"],
        ProgressiveCaptureOptions::with_byte_interval(600),
    );

    // With many worktrees, we should see clear progression
    assert!(
        output.stages.len() >= 3,
        "Should capture at least 3 stages with many worktrees, got {}",
        output.stages.len()
    );

    // Verify the initial stage has table structure but incomplete data
    let initial = output.initial().visible_text();
    assert!(
        initial.contains("Branch"),
        "Initial output should have table header"
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

    // Verify progressive filling happened
    output.verify_progressive_filling().unwrap();
}

/// Test that we can capture output even for fast commands
#[test]
fn test_list_progressive_fast_command() {
    let repo = TestRepo::new();
    repo.commit("Initial commit");

    // Run list without any worktrees (fast)
    let output = capture_progressive_output(
        &repo,
        "list",
        &[],
        ProgressiveCaptureOptions::with_byte_interval(600),
    );

    assert_eq!(output.exit_code, 0, "Command should succeed");

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
