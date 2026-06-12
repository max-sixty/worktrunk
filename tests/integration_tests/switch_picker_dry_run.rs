//! Integration tests for the picker's `WORKTRUNK_PICKER_DRY_RUN` path.
//!
//! Setting the env var bypasses skim entirely: the picker runs the full
//! pre-compute pipeline (speculative first-item spawn, collect, full spawn
//! loop, summaries), waits for all tasks, prints the rendered rows and the
//! preview-cache inventory as JSON, and exits. This exercises the non-TUI
//! wiring inside `handle_picker` without needing a PTY.
//!
//! Unix-only: `wt switch` rejects the interactive picker on Windows
//! ("Interactive picker is not available on Windows") before the dry-run
//! bypass is consulted.

#![cfg(unix)]

use crate::common::{TEST_EPOCH, TestRepo, repo};
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

/// Cached CI statuses surface PR numbers in picker rows without any network
/// access: the picker skips the CiStatus task, and collect fills `pr_status`
/// from `.git/wt/cache/ci-status/` instead. A fresh entry renders its
/// number; a branch without one renders an empty CI cell.
#[rstest]
fn test_picker_dry_run_shows_cached_pr_numbers(mut repo: TestRepo) {
    repo.add_worktree("feature-a");
    repo.add_worktree("feature-b");

    // Seed the cache the way a previous `wt list --full` fetch would have:
    // a fresh entry (TEST_EPOCH is the subprocess's `epoch_now()`) for
    // feature-a's current HEAD.
    let head = repo.git_output(&["rev-parse", "feature-a"]);
    let cache_dir = repo.path().join(".git/wt/cache/ci-status");
    std::fs::create_dir_all(&cache_dir).unwrap();
    let entry = serde_json::json!({
        "status": {
            "ci_status": "passed",
            "source": "pr",
            "is_stale": false,
            "number": { "number": 123, "sigil": "#" },
        },
        "checked_at": TEST_EPOCH,
        "head": head.trim(),
        "branch": "feature-a",
    });
    std::fs::write(cache_dir.join("feature-a.json"), entry.to_string()).unwrap();

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
    let rows: Vec<&str> = parsed["rows"]
        .as_array()
        .expect("top-level `rows` array")
        .iter()
        .map(|r| r.as_str().expect("row is a string"))
        .collect();

    let feature_row = rows
        .iter()
        .find(|r| r.contains("feature-a"))
        .expect("feature-a row present");
    assert!(
        feature_row.contains("#123"),
        "cached PR number should render in the picker row, got: {feature_row}"
    );
    let other_row = rows
        .iter()
        .find(|r| r.contains("feature-b"))
        .expect("feature-b row present");
    assert!(
        !other_row.contains('#'),
        "uncached branch renders an empty CI cell, got: {other_row}"
    );
}

/// Verifies that warnings collect emits while running on the picker's bg
/// thread (here, the stale-default-branch warning) reach stderr — they're
/// stashed during the run and drained after the dry-run path joins the bg
/// thread. Direct eprintln from collect would corrupt skim's frame in real
/// runs.
#[rstest]
fn test_picker_dry_run_drains_stashed_warnings(mut repo: TestRepo) {
    repo.add_worktree("feature-a");
    // Persist a default branch that doesn't exist locally — collect's
    // opportunistic stale-default check turns this into a warning.
    repo.run_git(&["config", "worktrunk.default-branch", "nonexistent"]);

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

    let stderr = String::from_utf8(output.stderr).expect("stderr is utf-8");
    assert!(
        stderr.contains("Configured default branch") && stderr.contains("nonexistent"),
        "expected stale-default-branch warning on stderr, got: {stderr}"
    );
    assert!(
        stderr.contains("wt config state default-branch clear"),
        "expected reset hint on stderr, got: {stderr}"
    );
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

/// `WORKTRUNK_PREVIEW_BENCH=1` shares the dry-run early-exit path but
/// suppresses the JSON dump and stashed-warning drain so the benchmark
/// measures just the preview-pool workload (spawn → all preview tasks
/// drained). Asserts the env-gated branch produces no stdout/stderr,
/// keeping the bench's hot path I/O-free.
#[rstest]
fn test_picker_preview_bench_produces_no_output(mut repo: TestRepo) {
    repo.add_worktree("feature-a");
    // Persist a default branch that doesn't exist locally — under dry-run
    // this becomes a stale-default warning on stderr; under preview-bench
    // the warning must stay stashed so I/O isn't part of the measurement.
    repo.run_git(&["config", "worktrunk.default-branch", "nonexistent"]);

    let output = repo
        .wt_command()
        .args(["switch"])
        .env("WORKTRUNK_PREVIEW_BENCH", "1")
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "preview-bench should exit 0; stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.stdout.is_empty(),
        "preview-bench must not write to stdout; got: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(
        output.stderr.is_empty(),
        "preview-bench must not write to stderr; got: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}
