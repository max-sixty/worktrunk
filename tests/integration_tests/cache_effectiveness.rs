//! Tests that RepoCache prevents duplicate git commands.
//!
//! When `wt list` processes multiple worktrees, repo-wide values (is_bare,
//! default_branch, primary_remote, etc.) should be computed once and shared
//! via the cache. These tests verify that by running `wt list` with trace
//! logging and asserting that specific git commands appear at most once.
//!
//! Uses `wt_perf::create_repo` to set up realistic benchmark repos with
//! multiple worktrees, commits, and remotes.

use std::path::Path;
use std::process::Command;

use worktrunk::trace::{TraceEntry, TraceEntryKind, parse_lines};
use wt_perf::{RepoConfig, create_repo, isolate_cmd};

mod common {
    pub use crate::common::*;
}

/// Lightweight repo config for cache tests. Minimal history since we only
/// care about command counts, not performance.
fn cache_test_config(worktrees: usize) -> RepoConfig {
    RepoConfig {
        commits_on_main: 3,
        files: 3,
        worktrees,
        worktree_commits_ahead: 1,
        worktree_uncommitted_files: 1,
        ..RepoConfig::typical(worktrees)
    }
}

/// Count how many trace entries have a command string containing the given pattern.
fn count_commands_matching(entries: &[TraceEntry], pattern: &str) -> usize {
    entries
        .iter()
        .filter(|e| {
            matches!(&e.kind, TraceEntryKind::Command { command, .. } if command.contains(pattern))
        })
        .count()
}

/// Run `wt list` with trace logging enabled, returning parsed trace entries.
fn run_list_with_traces(repo_path: &Path) -> Vec<TraceEntry> {
    let mut cmd = Command::new(common::wt_bin());
    cmd.args(["list"]);
    cmd.current_dir(repo_path);
    isolate_cmd(&mut cmd, None);
    cmd.env("RUST_LOG", "debug");

    let output = cmd.output().expect("failed to run wt list");

    assert!(
        output.status.success(),
        "wt list failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    parse_lines(&stderr)
}

/// Repo-wide cached commands run at most once regardless of worktree count.
///
/// With a typical-4 repo (4 worktrees, 500 commits, 100 files, fake remote),
/// commands guarded by OnceCell in RepoCache should execute exactly once.
/// If any assertion fails, a code path is bypassing the cache.
#[test]
fn test_repo_wide_caches_prevent_duplicate_commands() {
    let temp = create_repo(&cache_test_config(4));
    let repo_path = temp.path().join("repo");

    let entries = run_list_with_traces(&repo_path);

    // --- OnceCell-cached repo-wide commands: should appear exactly once ---

    // is_bare() cache
    assert_eq!(
        count_commands_matching(&entries, "--is-bare-repository"),
        1,
        "is_bare() should run git rev-parse --is-bare-repository exactly once"
    );

    // default_branch() cache — first call detects (read config + remote detection),
    // subsequent calls return cached value without any git commands.
    // Match the read specifically (--get), not the write that caches the result.
    assert_eq!(
        count_commands_matching(&entries, "config --get worktrunk.default-branch"),
        1,
        "default_branch() should read worktrunk.default-branch config exactly once"
    );

    // primary_remote() cache
    assert_eq!(
        count_commands_matching(&entries, "checkout.defaultRemote"),
        1,
        "primary_remote() should check checkout.defaultRemote exactly once"
    );

    // worktree list (not cached, but only called once in list command)
    assert_eq!(
        count_commands_matching(&entries, "worktree list"),
        1,
        "git worktree list should run exactly once"
    );
}

/// With more worktrees, repo-wide command counts should stay constant.
///
/// Compares typical-1 (1 worktree) vs typical-6 (6 worktrees). Repo-wide
/// cached commands should have identical counts regardless of scale.
#[test]
fn test_cache_scales_with_worktree_count() {
    let temp_1 = create_repo(&cache_test_config(1));
    let repo_1 = temp_1.path().join("repo");

    let entries_1wt = run_list_with_traces(&repo_1);
    let bare_1 = count_commands_matching(&entries_1wt, "--is-bare-repository");
    let default_1 = count_commands_matching(&entries_1wt, "config --get worktrunk.default-branch");

    let temp_6 = create_repo(&cache_test_config(6));
    let repo_6 = temp_6.path().join("repo");

    let entries_6wt = run_list_with_traces(&repo_6);
    let bare_6 = count_commands_matching(&entries_6wt, "--is-bare-repository");
    let default_6 = count_commands_matching(&entries_6wt, "config --get worktrunk.default-branch");

    assert_eq!(
        bare_1, bare_6,
        "is_bare command count should not grow with worktree count ({bare_1} vs {bare_6})"
    );
    assert_eq!(
        default_1, default_6,
        "default_branch command count should not grow with worktree count ({default_1} vs {default_6})"
    );
}

/// Total command count should not grow superlinearly with worktree count.
///
/// Compares typical-1 vs typical-4 total commands. Per-worktree commands
/// grow linearly; repo-wide commands stay constant. Superlinear growth
/// would indicate an O(N²) regression.
#[test]
fn test_total_command_count_scales_linearly() {
    let temp_1 = create_repo(&cache_test_config(1));
    let repo_1 = temp_1.path().join("repo");

    let entries_1 = run_list_with_traces(&repo_1);
    let total_1 = entries_1
        .iter()
        .filter(|e| matches!(&e.kind, TraceEntryKind::Command { .. }))
        .count();

    let temp_4 = create_repo(&cache_test_config(4));
    let repo_4 = temp_4.path().join("repo");

    let entries_4 = run_list_with_traces(&repo_4);
    let total_4 = entries_4
        .iter()
        .filter(|e| matches!(&e.kind, TraceEntryKind::Command { .. }))
        .count();

    // Allow 5x growth for 4x worktrees (generous margin for per-worktree
    // commands plus constant overhead).
    assert!(
        total_4 <= total_1 * 5,
        "Command count grew superlinearly: {total_1} (1 wt) → {total_4} (4 wt)"
    );
}
