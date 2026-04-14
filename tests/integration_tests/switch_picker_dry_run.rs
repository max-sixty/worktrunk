//! Integration tests for the picker's `WORKTRUNK_PICKER_DRY_RUN` path.
//!
//! Setting the env var bypasses skim entirely: the picker runs the full
//! pre-compute pipeline (speculative first-item spawn, collect, full spawn
//! loop, summaries), waits for all tasks, prints the cache inventory as JSON,
//! and exits. This exercises the non-TUI wiring inside `handle_picker`
//! without needing a PTY.

use crate::common::{TestRepo, repo};
use rstest::rstest;

#[rstest]
fn test_picker_dry_run_dumps_cache_json(mut repo: TestRepo) {
    repo.add_worktree("feature-a");
    repo.add_worktree("feature-b");

    let output = repo
        .wt_command()
        .args(["switch"])
        .env("WORKTRUNK_PICKER_DRY_RUN", "1")
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "dry-run should exit 0; stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).expect("stdout is utf-8");
    let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("stdout is valid JSON");
    let entries = parsed["entries"]
        .as_array()
        .expect("top-level `entries` array");

    assert!(
        !entries.is_empty(),
        "expected at least one cache entry, got: {stdout}"
    );

    // Every entry has {branch: string, mode: u8, bytes: usize}. Asserting
    // schema (not specific branches/modes) keeps the test robust to fixture
    // changes while still covering the dump format.
    for e in entries {
        assert!(e["branch"].is_string(), "entry missing branch: {e}");
        assert!(e["mode"].is_number(), "entry missing mode: {e}");
        assert!(e["bytes"].is_number(), "entry missing bytes: {e}");
    }
}
