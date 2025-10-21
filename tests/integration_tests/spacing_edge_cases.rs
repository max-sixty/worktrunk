use crate::common::TestRepo;
use insta::Settings;
use insta_cmd::{assert_cmd_snapshot, get_cargo_bin};
use std::process::Command;

/// Helper to create snapshot with normalized paths and SHAs
fn snapshot_list(test_name: &str, repo: &TestRepo) {
    let mut settings = Settings::clone_current();
    settings.set_snapshot_path("../snapshots");

    // Normalize paths
    settings.add_filter(repo.root_path().to_str().unwrap(), "[REPO]");
    for (name, path) in &repo.worktrees {
        settings.add_filter(
            path.to_str().unwrap(),
            format!("[WORKTREE_{}]", name.to_uppercase().replace('-', "_")),
        );
    }

    // Normalize git SHAs
    settings.add_filter(r"\b[0-9a-f]{7,40}\b", "[SHA]   ");
    settings.add_filter(r"\\", "/");

    settings.bind(|| {
        let mut cmd = Command::new(get_cargo_bin("wt"));
        repo.clean_cli_env(&mut cmd);
        cmd.arg("list").current_dir(repo.root_path());
        assert_cmd_snapshot!(test_name, cmd);
    });
}

#[test]
fn test_short_branch_names_shorter_than_header() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit");

    // Create worktrees with very short branch names (shorter than "Branch" header)
    repo.add_worktree("a", "a");
    repo.add_worktree("bb", "bb");
    repo.add_worktree("ccc", "ccc");

    snapshot_list("short_branch_names", &repo);
}

#[test]
fn test_long_branch_names_longer_than_header() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit");

    // Create worktrees with very long branch names
    repo.add_worktree(
        "very-long-feature-branch-name",
        "very-long-feature-branch-name",
    );
    repo.add_worktree(
        "another-extremely-long-name-here",
        "another-extremely-long-name-here",
    );
    repo.add_worktree("short", "short");

    snapshot_list("long_branch_names", &repo);
}

#[test]
fn test_unicode_branch_names_width_calculation() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit");

    // Create worktrees with unicode characters that have different visual widths
    // Note: Git may have restrictions on branch names, so use valid characters
    repo.add_worktree("café", "cafe");
    repo.add_worktree("naïve", "naive");
    repo.add_worktree("résumé", "resume");

    snapshot_list("unicode_branch_names", &repo);
}

#[test]
fn test_mixed_length_branch_names() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit");

    // Mix of very short, medium, and very long branch names
    repo.add_worktree("x", "x");
    repo.add_worktree("medium-length-name", "medium");
    repo.add_worktree(
        "extremely-long-branch-name-that-might-cause-layout-issues",
        "long",
    );

    snapshot_list("mixed_length_branch_names", &repo);
}

/// Helper for testing with specific terminal width
fn snapshot_list_with_width(test_name: &str, repo: &TestRepo, width: usize) {
    let mut settings = Settings::clone_current();
    settings.set_snapshot_path("../snapshots");

    // Normalize paths
    settings.add_filter(repo.root_path().to_str().unwrap(), "[REPO]");
    for (name, path) in &repo.worktrees {
        settings.add_filter(
            path.to_str().unwrap(),
            format!("[WORKTREE_{}]", name.to_uppercase().replace('-', "_")),
        );
    }

    // Normalize git SHAs
    settings.add_filter(r"\b[0-9a-f]{7,40}\b", "[SHA]   ");
    settings.add_filter(r"\\", "/");

    settings.bind(|| {
        let mut cmd = Command::new(get_cargo_bin("wt"));
        repo.clean_cli_env(&mut cmd);
        cmd.arg("list")
            .current_dir(repo.root_path())
            .env("COLUMNS", width.to_string());
        assert_cmd_snapshot!(test_name, cmd);
    });
}

#[test]
fn test_terminal_width_80_drops_message() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit with a reasonably long message");

    repo.add_worktree("feature-a", "feature-a");
    repo.add_worktree("feature-b", "feature-b");

    snapshot_list_with_width("terminal_width_80", &repo, 80);
}

#[test]
fn test_terminal_width_120_shows_message() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit with a reasonably long message");

    repo.add_worktree("feature-a", "feature-a");
    repo.add_worktree("feature-b", "feature-b");

    snapshot_list_with_width("terminal_width_120", &repo, 120);
}

#[test]
fn test_terminal_width_150_shows_all_columns() {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit with a reasonably long message");

    repo.add_worktree("feature-a", "feature-a");
    repo.add_worktree("feature-b", "feature-b");

    snapshot_list_with_width("terminal_width_150", &repo, 150);
}
