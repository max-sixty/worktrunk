//! Integration tests for the picker's `WORKTRUNK_PICKER_DRY_RUN` path.
//!
//! Setting the env var bypasses skim entirely: the picker runs the full
//! pre-compute pipeline (speculative first-item spawn, collect, full spawn
//! loop, summaries), waits for all tasks, prints the rendered rows and the
//! preview-cache inventory as JSON, and exits. This exercises the non-TUI
//! wiring inside `handle_picker` without needing a PTY.
//!
//! Runs on every platform: the dry-run bypass is consulted before the
//! interactive TTY path, so these exercise the picker pipeline on Windows too.

use crate::common::mock_commands::{MockConfig, MockResponse, mock_calls};
use crate::common::{TEST_EPOCH, TestRepo, repo};
use rstest::rstest;

#[rstest::fixture]
fn main_only_repo() -> TestRepo {
    TestRepo::standard_main_only()
}

/// Install a strict fake summary command through TestRepo's cross-platform
/// mock-PATH wiring. Tests observe its external call log to distinguish the
/// generated-summary route from the static mode-5 config hint.
fn install_summary_probe(repo: &mut TestRepo) {
    repo.setup_mock_gh();
    let mock_bin = repo
        .mock_bin_path()
        .expect("setup_mock_gh installs a mock bin");
    MockConfig::new("summary-probe")
        .command("_default", MockResponse::output("generated-summary-marker"))
        .write(mock_bin);
}

/// Runs the dry-run picker with summaries disabled (default). Covers configured
/// branch-only row collection, the base cache-dump path, and the `else` branch
/// that inserts a config hint in place of real summaries.
#[rstest]
fn test_picker_dry_run_emits_configured_rows_and_cache_json(
    #[from(main_only_repo)] mut repo: TestRepo,
) {
    repo.run_git(&["remote", "remove", "origin"]);
    repo.add_worktree("active-worktree");
    repo.run_git(&["branch", "orphan-branch"]);

    let orphan_head = repo.git_output(&["rev-parse", "--verify", "refs/heads/orphan-branch"]);
    assert_eq!(
        orphan_head,
        repo.git_output(&["rev-parse", "HEAD"]),
        "orphan-branch must exist as a local branch ref"
    );
    let worktrees = repo.git_output(&["worktree", "list", "--porcelain"]);
    assert!(
        !worktrees
            .lines()
            .any(|line| line == "branch refs/heads/orphan-branch"),
        "orphan-branch must not belong to a worktree:\n{worktrees}"
    );

    repo.write_test_config("[list]\nbranches = true\n");
    install_summary_probe(&mut repo);
    let call_log = tempfile::tempdir().unwrap();

    let output = repo
        .wt_command()
        // No `--branches`: the config setting must drive branch collection.
        .args(["switch"])
        .env("WORKTRUNK_PICKER_DRY_RUN", "1")
        // A configured generator must still stay off while list.summary=false.
        .env("WORKTRUNK_COMMIT__GENERATION__COMMAND", "summary-probe")
        .env("WORKTRUNK_TEST_MOCK_CALL_LOG_DIR", call_log.path())
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
        .map(|row| row.as_str().expect("row is a string"))
        .collect();
    assert!(
        rows.iter().any(|row| row.contains("/ orphan-branch")),
        "configured branch-only row missing from structured rows: {rows:#?}"
    );

    let entries = parsed["entries"]
        .as_array()
        .expect("top-level `entries` array");

    assert!(
        !entries.is_empty(),
        "expected at least one cache entry, got: {stdout}"
    );

    // Every entry has {branch: string, mode: u8, bytes: usize, content: string}. Asserting
    // schema (not specific branches/modes) keeps the test robust to fixture
    // changes while still covering the dump format.
    for e in entries {
        assert!(
            e["branch"]
                .as_str()
                .is_some_and(|branch| !branch.is_empty()),
            "entry missing nonempty branch: {e}"
        );
        assert!(
            e["mode"]
                .as_u64()
                .is_some_and(|mode| u8::try_from(mode).is_ok()),
            "entry mode is not a u8: {e}"
        );
        assert!(
            e["bytes"]
                .as_u64()
                .is_some_and(|bytes| usize::try_from(bytes).is_ok()),
            "entry bytes is not a usize: {e}"
        );
        assert!(
            e["content"].is_string(),
            "entry content is not a string: {e}"
        );
    }

    assert!(
        entries
            .iter()
            .any(|e| e["mode"] == 6 && e["bytes"].as_u64().is_some_and(|bytes| bytes > 0)),
        "summaries are disabled, so the picker must seed a nonempty static config hint: {stdout}"
    );
    assert!(
        mock_calls(call_log.path(), "summary-probe").is_empty(),
        "a configured summary command must not run while list.summary=false"
    );
}

/// The speculative default-preview producer runs before list collection. It
/// must carry a real HEAD, or it can poison the `(branch, UnifiedDiff)` cache
/// key with an error pane before the collected row tries to fill it. One Rayon
/// worker makes that ordering deterministic. The hidden-untracked setting also
/// pins that the shared cleanliness signal cannot suppress the file.
#[rstest]
fn test_picker_dry_run_speculative_complete_diff_uses_real_head(
    #[from(main_only_repo)] repo: TestRepo,
) {
    repo.run_git(&["config", "status.showUntrackedFiles", "no"]);
    std::fs::write(repo.path().join("untracked-preview.txt"), "preview\n").unwrap();
    let branch = repo.current_branch();

    let output = repo
        .wt_command()
        .args(["switch"])
        .env("RAYON_NUM_THREADS", "1")
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
    let complete = parsed["entries"]
        .as_array()
        .expect("top-level `entries` array")
        .iter()
        .find(|entry| entry["branch"] == branch && entry["mode"] == 1)
        .expect("current branch has a complete-diff cache entry");
    let content = complete["content"].as_str().expect("content is a string");

    assert!(
        content.contains("untracked-preview.txt"),
        "complete preview should contain the untracked file, got: {content:?}"
    );
    assert!(
        !content.contains("Could not load the complete diff"),
        "speculative preview must not cache an error pane: {content:?}"
    );
}

/// A user may point TMPDIR inside the repository (common in hermetic build
/// environments). Temporary indexes share an excluded temp namespace so
/// concurrent panes cannot see them as untracked files. Genuine untracked
/// content still appears.
#[rstest]
fn test_picker_dry_run_tempdir_inside_worktree_has_no_index_artifacts(
    #[from(main_only_repo)] repo: TestRepo,
) {
    // Brackets also prove the exclusion quotes Git pathspec glob metacharacters
    // in the user-selected temporary directory.
    let local_tmp = repo.path().join("local-[tmp]");
    std::fs::create_dir_all(&local_tmp).unwrap();
    std::fs::write(local_tmp.join(".gitkeep"), "").unwrap();
    repo.run_git(&["add", "local-[tmp]/.gitkeep"]);
    repo.run_git(&["commit", "-m", "track local temp directory"]);
    std::fs::write(repo.path().join("loose.txt"), "loose\n").unwrap();
    let branch = repo.current_branch();

    let output = repo
        .wt_command()
        .args(["switch"])
        .env("TMPDIR", &local_tmp)
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
    for mode in [1, 2] {
        let pane = entries
            .iter()
            .find(|entry| entry["branch"] == branch && entry["mode"] == mode)
            .unwrap_or_else(|| panic!("branch {branch} has mode-{mode} cache entry"));
        let content = pane["content"].as_str().expect("content is a string");
        assert!(
            content.contains("loose.txt"),
            "mode {mode} retains genuine untracked content: {content:?}"
        );
        assert!(
            !content.contains("local-[tmp]/"),
            "mode {mode} must not include temporary-index artifacts: {content:?}"
        );
    }
}

/// Intent-to-add must cross a sparse-checkout boundary. Otherwise one
/// untracked file outside the sparse definition makes Git reject the whole add
/// and both complete/working panes degrade to an unavailable warning.
#[rstest]
fn test_picker_dry_run_includes_untracked_outside_sparse_checkout(
    #[from(main_only_repo)] repo: TestRepo,
) {
    let visible = repo.path().join("visible");
    let hidden = repo.path().join("hidden");
    std::fs::create_dir_all(&visible).unwrap();
    std::fs::create_dir_all(&hidden).unwrap();
    std::fs::write(visible.join("tracked.txt"), "base\n").unwrap();
    std::fs::write(hidden.join("tracked.txt"), "base\n").unwrap();
    repo.run_git(&["add", "visible/tracked.txt", "hidden/tracked.txt"]);
    repo.run_git(&["commit", "-m", "add sparse fixture"]);
    repo.run_git(&["sparse-checkout", "init", "--cone"]);
    repo.run_git(&["sparse-checkout", "set", "visible"]);

    std::fs::write(visible.join("tracked.txt"), "changed\n").unwrap();
    std::fs::create_dir_all(&hidden).unwrap();
    std::fs::write(hidden.join("loose.txt"), "loose\n").unwrap();
    let branch = repo.current_branch();

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
    for mode in [1, 2] {
        let pane = entries
            .iter()
            .find(|entry| entry["branch"] == branch && entry["mode"] == mode)
            .unwrap_or_else(|| panic!("branch {branch} has mode-{mode} cache entry"));
        let content = pane["content"].as_str().expect("content is a string");
        assert!(
            content.contains("visible/tracked.txt") && content.contains("hidden/loose.txt"),
            "mode {mode} includes tracked and out-of-sparse untracked changes: {content:?}"
        );
        assert!(
            !content.contains("Could not load"),
            "mode {mode} must not degrade on sparse checkout: {content:?}"
        );
    }
}

/// `switch --prs` must select the GitLab subprocess route from the remote URL,
/// not merely parse GitLab-shaped JSON after some caller has already selected
/// the provider. A fresh null CI cache entry keeps the worktree-row CI task
/// local, leaving only the `--prs` availability probe and list call observable.
#[rstest]
fn test_picker_dry_run_prs_routes_to_gitlab(#[from(main_only_repo)] mut repo: TestRepo) {
    repo.run_git(&[
        "remote",
        "set-url",
        "origin",
        "https://gitlab.com/owner/test-repo.git",
    ]);

    let branch = repo.current_branch();
    let head = repo.git_output(&["rev-parse", "HEAD"]);
    let cache_dir = repo.path().join(".git/wt/cache/ci-status");
    std::fs::create_dir_all(&cache_dir).unwrap();
    let entry = serde_json::json!({
        "status": null,
        "checked_at": TEST_EPOCH,
        "head": head.trim(),
        "branch": branch,
    });
    std::fs::write(cache_dir.join(format!("{branch}.json")), entry.to_string()).unwrap();

    // Reuse TestRepo's cross-platform mock-PATH wiring, then replace both forge
    // configs with strict route-specific mocks.
    repo.setup_mock_gh();
    let mock_bin = repo
        .mock_bin_path()
        .expect("setup_mock_gh installs a mock bin")
        .to_path_buf();
    MockConfig::new("glab")
        .version("glab version 1.0.0 (mock)")
        .command(
            "mr list --per-page 50 --output json",
            MockResponse::output("[]"),
        )
        .command("_default", MockResponse::exit(1))
        .write(&mock_bin);
    MockConfig::new("gh")
        .command("_default", MockResponse::exit(1))
        .write(&mock_bin);

    // Keep observability outside the repository: writing the call log into the
    // worktree would perturb the status being collected by the picker.
    let call_log = tempfile::tempdir().unwrap();
    let output = repo
        .wt_command()
        .args(["switch", "--prs"])
        .env("WORKTRUNK_PICKER_DRY_RUN", "1")
        .env("WORKTRUNK_TEST_MOCK_CALL_LOG_DIR", call_log.path())
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "GitLab dry-run should exit 0; stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        mock_calls(call_log.path(), "glab"),
        [
            "--version".to_string(),
            "mr list --per-page 50 --output json".to_string(),
        ],
        "GitLab routing must issue exactly the availability probe and MR list call"
    );
    assert!(
        mock_calls(call_log.path(), "gh").is_empty(),
        "a GitLab remote must not invoke the GitHub CLI"
    );
}

/// A fresh cached CI status surfaces a PR number in picker rows without any
/// network access: the picker primes rows from `.git/wt/cache/ci-status/`, and
/// the live `CiStatus` task's `detect` reads the same fresh entry through its
/// own cache rather than calling the forge. A branch without an entry renders
/// an empty CI cell.
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

    // The worktree row with a PR spawns a `comments` background fetch keyed by
    // its branch name (PreviewMode::Comments == 8) — the same fetch a `--prs`
    // row makes, so the comments tab is no longer `--prs`-only. In this no-network
    // test the entry holds a terminal pane (a "couldn't load" on a GitHub remote,
    // or a "forge unsupported" note otherwise), but its presence proves the fetch
    // fired and keyed by branch. The PR-less row spawns no such fetch.
    let comments_branches: Vec<&str> = parsed["entries"]
        .as_array()
        .expect("top-level `entries` array")
        .iter()
        .filter(|e| e["mode"] == 8)
        .map(|e| e["branch"].as_str().expect("branch is a string"))
        .collect();
    assert!(
        comments_branches.contains(&"feature-a"),
        "the PR-bearing worktree row spawns a branch-keyed comments fetch, got: {comments_branches:?}"
    );
    assert!(
        !comments_branches.contains(&"feature-b"),
        "the PR-less worktree row spawns no comments fetch, got: {comments_branches:?}"
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

/// Same as above but with `list.summary=true` and a strict fake LLM command,
/// proving the configured generator runs before its result reaches mode 6.
#[rstest]
fn test_picker_dry_run_with_summary(mut repo: TestRepo) {
    repo.add_worktree("feature-a");
    install_summary_probe(&mut repo);
    let call_log = tempfile::tempdir().unwrap();

    let output = repo
        .wt_command()
        .args(["switch"])
        .env("WORKTRUNK_PICKER_DRY_RUN", "1")
        .env("WORKTRUNK_LIST__SUMMARY", "true")
        .env("WORKTRUNK_COMMIT__GENERATION__COMMAND", "summary-probe")
        .env("WORKTRUNK_TEST_MOCK_CALL_LOG_DIR", call_log.path())
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

    // Summary mode is `6` (see `PreviewMode` in `src/commands/picker/preview.rs`).
    // At least one entry should be a summary when summaries are enabled —
    // that proves the summary spawn branch ran to completion. Mode 5
    // (UpstreamDiff) is always present as part of the normal preview
    // modes array, so asserting on it would not prove anything about
    // the summary path.
    assert!(
        entries.iter().any(|e| e["mode"] == 6),
        "expected at least one Summary entry, got: {stdout}"
    );
    assert!(
        !mock_calls(call_log.path(), "summary-probe").is_empty(),
        "list.summary=true must invoke the configured summary command"
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

/// The picker renders custom `[list.custom-columns]` cells in its skeleton
/// rows — the `ColumnKind::Custom` arm of `render_skeleton_row`, which the
/// piped `wt list` snapshot tests never reach because they skip the skeleton.
#[rstest]
fn test_picker_dry_run_renders_custom_columns(mut repo: TestRepo) {
    repo.add_worktree("feature-a");
    std::fs::write(
        repo.test_config_path(),
        r#"[list.custom-columns.Ticket]
template = "{{ vars.ticket }}"
"#,
    )
    .unwrap();
    repo.run_git(&[
        "config",
        "worktrunk.state.feature-a.vars.ticket",
        "JIRA-1234",
    ]);

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
        feature_row.contains("JIRA-1234"),
        "custom column value renders in the picker skeleton row, got: {feature_row}"
    );
}

/// A broken custom-column template aborts `wt list`, but the picker shares the
/// collect path while skim owns the terminal and can't surface an abort: it
/// stashes a warning (drained after close) and renders without the column.
/// Covers the `progressive_handler` degradation arm in `collect`.
#[rstest]
fn test_picker_dry_run_invalid_custom_column_degrades(mut repo: TestRepo) {
    repo.add_worktree("feature-a");
    std::fs::write(
        repo.test_config_path(),
        r#"[list.custom-columns.Ticket]
template = "{{ branhc }}"
"#,
    )
    .unwrap();

    let output = repo
        .wt_command()
        .args(["switch"])
        .env("WORKTRUNK_PICKER_DRY_RUN", "1")
        .output()
        .unwrap();

    // The picker degrades rather than aborting: it still exits 0 and renders
    // its rows, with the broken column dropped.
    assert!(
        output.status.success(),
        "picker degrades on a broken column instead of aborting; stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stderr = String::from_utf8(output.stderr).expect("stderr is utf-8");
    assert!(
        stderr.contains("Custom columns disabled"),
        "expected a stashed degradation warning on stderr, got: {stderr}"
    );

    let stdout = String::from_utf8(output.stdout).expect("stdout is utf-8");
    let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("stdout is valid JSON");
    let rows = parsed["rows"].as_array().expect("top-level `rows` array");
    assert!(
        rows.iter()
            .any(|r| r.as_str().unwrap_or("").contains("feature-a")),
        "rows still render without the broken column, got: {stdout}"
    );
}
