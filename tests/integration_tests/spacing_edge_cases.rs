use crate::common::{TestRepo, list_snapshots, repo, setup_snapshot_settings};
use insta::Settings;
use insta_cmd::assert_cmd_snapshot;
use rstest::rstest;
use std::process::Command;

fn snapshot_list(test_name: &str, repo: &TestRepo) {
    run_snapshot(
        setup_snapshot_settings(repo),
        test_name,
        list_snapshots::command(repo, repo.root_path()),
    );
}

fn snapshot_list_with_width(test_name: &str, repo: &TestRepo, width: usize) {
    run_snapshot(
        setup_snapshot_settings(repo),
        test_name,
        list_snapshots::command_with_width(repo, width),
    );
}

fn run_snapshot(settings: Settings, test_name: &str, mut cmd: Command) {
    settings.bind(|| {
        assert_cmd_snapshot!(test_name, cmd);
    });
}

#[rstest]
fn test_short_branch_names_shorter_than_header(mut repo: TestRepo) {
    // Create worktrees with very short branch names (shorter than "Branch" header)
    repo.add_worktree("a");
    repo.add_worktree("bb");
    repo.add_worktree("ccc");

    snapshot_list("short_branch_names", &repo);
}

#[rstest]
fn test_long_branch_names_longer_than_header(mut repo: TestRepo) {
    // Create worktrees with very long branch names
    repo.add_worktree("very-long-feature-branch-name");
    repo.add_worktree("another-extremely-long-name-here");
    repo.add_worktree("short");

    snapshot_list("long_branch_names", &repo);
}

#[rstest]
fn test_unicode_branch_names_width_calculation(mut repo: TestRepo) {
    // Create worktrees with unicode characters that have different visual widths
    // Note: Git may have restrictions on branch names, so use valid characters
    repo.add_worktree("cafe");
    repo.add_worktree("naive");
    repo.add_worktree("resume");

    snapshot_list("unicode_branch_names", &repo);
}

#[rstest]
fn test_mixed_length_branch_names(mut repo: TestRepo) {
    // Mix of very short, medium, and very long branch names
    repo.add_worktree("x");
    repo.add_worktree("medium");
    repo.add_worktree("extremely-long-branch-name-that-might-cause-layout-issues");

    snapshot_list("mixed_length_branch_names", &repo);
}

// Column alignment tests with varying diff sizes
// (Merged from column_alignment.rs)

#[rstest]
fn test_column_alignment_varying_diff_widths(mut repo: TestRepo) {
    // Create worktrees with varying diff sizes to test alignment
    repo.add_worktree("feature-small");
    repo.add_worktree("feature-medium");
    repo.add_worktree("feature-large");

    // Add files to create diffs with different digit counts
    let small_path = repo.worktrees.get("feature-small").unwrap();
    for i in 0..5 {
        std::fs::write(small_path.join(format!("file{}.txt", i)), "content").unwrap();
    }

    let medium_path = repo.worktrees.get("feature-medium").unwrap();
    for i in 0..50 {
        std::fs::write(medium_path.join(format!("file{}.txt", i)), "content").unwrap();
    }

    let large_path = repo.worktrees.get("feature-large").unwrap();
    for i in 0..500 {
        std::fs::write(large_path.join(format!("file{}.txt", i)), "content").unwrap();
    }

    // Test at a width where Dirty column is visible
    snapshot_list_with_width("alignment_varying_diffs", &repo, 180);
}

#[rstest]
fn test_column_alignment_with_empty_diffs(mut repo: TestRepo) {
    // Mix of worktrees with and without diffs
    repo.add_worktree("no-changes");

    repo.add_worktree("with-changes");
    let changes_path = repo.worktrees.get("with-changes").unwrap();
    std::fs::write(changes_path.join("file.txt"), "content").unwrap();

    repo.add_worktree("also-no-changes");

    // Path column should align even when some rows have diffs and others don't
    snapshot_list_with_width("alignment_empty_diffs", &repo, 180);
}

#[rstest]
fn test_column_alignment_extreme_diff_sizes(mut repo: TestRepo) {
    // Create worktrees with extreme diff size differences
    repo.add_worktree("tiny");
    repo.add_worktree("huge");

    let tiny_path = repo.worktrees.get("tiny").unwrap();
    std::fs::write(tiny_path.join("file.txt"), "x").unwrap();

    let huge_path = repo.worktrees.get("huge").unwrap();
    // 200 files is enough to test alignment with 3-digit diff counts
    for i in 0..200 {
        std::fs::write(huge_path.join(format!("file{}.txt", i)), "content").unwrap();
    }

    snapshot_list_with_width("alignment_extreme_diffs", &repo, 180);
}
