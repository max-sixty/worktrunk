//! Integration tests for the picker's `WORKTRUNK_PICKER_DRY_RUN` path.
//!
//! Setting the env var bypasses skim entirely: the picker runs the full
//! pre-compute pipeline (speculative first-item spawn, collect, full spawn
//! loop, summaries), waits for all tasks, prints the cache inventory as JSON,
//! and exits. This exercises the non-TUI wiring inside `handle_picker`
//! without needing a PTY.
//!
//! Unix-only: `wt switch` rejects the interactive picker on Windows
//! ("Interactive picker is not available on Windows") before the dry-run
//! bypass is consulted.

#![cfg(unix)]

use crate::common::{TestRepo, repo};
use rstest::rstest;

/// Runs the dry-run picker with summaries disabled (default). Covers the
/// base cache-dump path and the `else` branch that inserts a config hint
/// in place of real summaries.
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

/// Same as above but with `list.summary=true` and a fake LLM command
/// configured, to exercise the `spawn_summary` branch in `handle_picker`.
/// Uses `/bin/cat` as the LLM: it reads stdin and writes it back, so the
/// summary pipeline runs end-to-end without a real model.
#[rstest]
fn test_picker_dry_run_with_summary(mut repo: TestRepo) {
    repo.add_worktree("feature-a");

    let output = repo
        .wt_command()
        .args(["switch"])
        .env("WORKTRUNK_PICKER_DRY_RUN", "1")
        .env("WORKTRUNK_LIST__SUMMARY", "true")
        .env("WORKTRUNK_COMMIT__GENERATION__COMMAND", "/bin/cat")
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

    // Summary mode is `5` (see `PreviewMode` in `src/commands/picker/preview.rs`).
    // At least one entry should be a summary when summaries are enabled —
    // that proves the summary spawn branch ran to completion. Mode 4
    // (UpstreamDiff) is always present as part of the normal preview
    // modes array, so asserting on it would not prove anything about
    // the summary path.
    assert!(
        entries.iter().any(|e| e["mode"] == 5),
        "expected at least one Summary entry, got: {stdout}"
    );
}
