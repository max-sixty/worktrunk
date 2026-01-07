//! Tests for verifying column alignment in list output
//!
//! These tests ensure that column headers align with their data,
//! and that progressive rendering maintains consistent alignment.

use crate::common::{TestRepo, make_snapshot_cmd, repo};
use insta_cmd::assert_cmd_snapshot;
use rstest::rstest;

#[rstest]
fn test_status_column_alignment_with_header(mut repo: TestRepo) {
    // Create worktree with status symbols
    let wt = repo.add_worktree_with_commit("test", "file.txt", "content", "Test");

    // Add working tree changes for Status symbols
    std::fs::write(wt.join("untracked.txt"), "new").unwrap();
    std::fs::write(wt.join("file.txt"), "modified").unwrap();

    assert_cmd_snapshot!(make_snapshot_cmd(&repo, "list", &[], None));
}

#[rstest]
fn test_status_column_width_consistency(mut repo: TestRepo) {
    // Create multiple worktrees with different status symbol combinations
    let wt1 = repo.add_worktree_with_commit("simple", "file.txt", "content", "Simple");
    let wt2 = repo.add_worktree_with_commit("complex", "file.txt", "content", "Complex");

    // Add different working tree changes
    std::fs::write(wt1.join("new.txt"), "new").unwrap(); // Just untracked (?)
    std::fs::write(wt2.join("new1.txt"), "new").unwrap(); // Multiple: ?, !, +
    std::fs::write(wt2.join("file.txt"), "modified").unwrap();
    repo.run_git_in(&wt2, &["add", "file.txt"]);

    assert_cmd_snapshot!(make_snapshot_cmd(&repo, "list", &[], None));
}
