//! Tests for verifying column alignment in list output
//!
//! These tests ensure that column headers align with their data,
//! and that progressive rendering maintains consistent alignment.

use crate::common::{TestRepo, make_snapshot_cmd, setup_snapshot_settings};
use insta_cmd::assert_cmd_snapshot;

/// Test that Status column data aligns with Status header
#[test]
fn test_status_column_alignment_with_header() {
    let mut repo = TestRepo::new();

    // Create worktree with status symbols
    let wt = repo.add_worktree("test");
    std::fs::write(wt.join("file.txt"), "content").unwrap();

    let mut cmd = std::process::Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["add", "file.txt"])
        .current_dir(&wt)
        .output()
        .unwrap();
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["commit", "-m", "Test"])
        .current_dir(&wt)
        .output()
        .unwrap();

    // Add working tree changes for Status symbols
    std::fs::write(wt.join("untracked.txt"), "new").unwrap();
    std::fs::write(wt.join("file.txt"), "modified").unwrap();

    let settings = setup_snapshot_settings(&repo);
    settings.bind(|| {
        let mut cmd = make_snapshot_cmd(&repo, "list", &[], None);
        assert_cmd_snapshot!("status_column_alignment_check", cmd);
    });
}

/// Test that Status column width is consistent across all rows
#[test]
fn test_status_column_width_consistency() {
    let mut repo = TestRepo::new();

    // Create multiple worktrees with different status symbol combinations
    let wt1 = repo.add_worktree("simple");
    std::fs::write(wt1.join("file.txt"), "content").unwrap();

    let mut cmd = std::process::Command::new("git");
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["add", "file.txt"])
        .current_dir(&wt1)
        .output()
        .unwrap();
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["commit", "-m", "Simple"])
        .current_dir(&wt1)
        .output()
        .unwrap();

    let wt2 = repo.add_worktree("complex");
    std::fs::write(wt2.join("file.txt"), "content").unwrap();
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["add", "file.txt"])
        .current_dir(&wt2)
        .output()
        .unwrap();
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["commit", "-m", "Complex"])
        .current_dir(&wt2)
        .output()
        .unwrap();

    // Add different working tree changes
    std::fs::write(wt1.join("new.txt"), "new").unwrap(); // Just untracked (?)
    std::fs::write(wt2.join("new1.txt"), "new").unwrap(); // Multiple: ?, !, +
    std::fs::write(wt2.join("file.txt"), "modified").unwrap();
    repo.configure_git_cmd(&mut cmd);
    cmd.args(["add", "file.txt"])
        .current_dir(&wt2)
        .output()
        .unwrap();

    let settings = setup_snapshot_settings(&repo);
    settings.bind(|| {
        let mut cmd = make_snapshot_cmd(&repo, "list", &[], None);
        assert_cmd_snapshot!("status_column_width_consistency", cmd);
    });
}
