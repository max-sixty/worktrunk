//! Tests for diagnostic report generation.
//!
//! These tests verify the markdown structure and content of diagnostic reports
//! to ensure they're suitable for GitHub issue filing.
//!
//! # Test Coverage
//!
//! - `test_diagnostic_report_file_format`: Snapshot of full diagnostic structure
//! - `test_diagnostic_not_created_without_vv`: No file without -vv
//! - `test_diagnostic_hint_without_vv`: Hint tells user to use -vv
//! - `test_diagnostic_contains_required_sections`: All sections present
//! - `test_diagnostic_context_has_no_ansi_codes`: ANSI stripped for GitHub
//! - `test_diagnostic_trace_log_contains_git_commands`: Log has useful data
//! - `test_diagnostic_saved_message_with_vv`: Output shows "Diagnostic saved" with -vv
//! - `test_diagnostic_written_to_correct_location`: File in .git/wt/logs/
//! - `test_diagnostic_gh_hint_with_vv`: Hint shows gist and issue URL when gh installed

use std::fs;
use std::path::PathBuf;

use insta::assert_snapshot;
use rstest::rstest;

use crate::common::{TestRepo, repo, setup_snapshot_settings};

/// Helper to corrupt a worktree's HEAD file to trigger git errors.
///
/// Writes a non-null invalid SHA so the worktree is treated as "valid but
/// broken" (errors surface) rather than the NULL_OID-keyed unborn path,
/// which now skips commit-dependent tasks silently (see #2936).
fn corrupt_worktree_head(repo: &TestRepo, worktree_name: &str) -> PathBuf {
    let feature_path = repo.worktrees.get(worktree_name).unwrap();
    let git_dir = feature_path.join(".git");
    let git_content = fs::read_to_string(&git_dir).unwrap();
    let actual_git_dir = git_content
        .strip_prefix("gitdir: ")
        .unwrap()
        .trim()
        .to_string();
    let head_path = PathBuf::from(&actual_git_dir).join("HEAD");
    fs::write(&head_path, "0000000000000000000000000000000000000001\n").unwrap();
    head_path
}

/// Snapshot the diagnostic report file generated with -vv.
///
/// This test triggers a git error (invalid HEAD) and runs `wt list -vv`
/// to generate a diagnostic report file. We then read and snapshot the file
/// to verify its structure.
///
/// Note: Diagnostic files are only generated when -vv is used.
#[rstest]
fn test_diagnostic_report_file_format(mut repo: TestRepo) {
    repo.add_worktree("feature");
    corrupt_worktree_head(&repo, "feature");

    let output = repo.wt_command().args(["list", "-vv"]).output().unwrap();

    let diagnostic_path = repo
        .root_path()
        .join(".git")
        .join("wt/logs")
        .join("diagnostic.md");
    assert!(
        diagnostic_path.exists(),
        "Diagnostic file should be generated at {:?}",
        diagnostic_path
    );

    let content = fs::read_to_string(&diagnostic_path).unwrap();

    // Verify verbose log section is present (requires -v or higher)
    assert!(
        content.contains("<summary>Trace log</summary>"),
        "Diagnostic should include trace log section when run with -vv"
    );

    // The rendered performance profile rides alongside the raw trace.
    assert!(
        content.contains("<summary>Performance profile</summary>"),
        "Diagnostic should include a performance profile section when run with -vv"
    );

    let settings = setup_snapshot_settings(&repo);
    settings.bind(|| {
        assert_snapshot!("diagnostic_file_format", normalize_report(&content));
    });

    // Verify the stderr mentions the diagnostic was saved
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Diagnostic saved"),
        "Output should mention diagnostic was saved. stderr: {}",
        stderr
    );
}

/// Without -vv, no diagnostic file should be created.
///
/// The diagnostic file is only created when -vv is used. Running without
/// any verbose flag or with just -v does not create a diagnostic file.
#[rstest]
fn test_diagnostic_not_created_without_vv(mut repo: TestRepo) {
    repo.add_worktree("feature");
    corrupt_worktree_head(&repo, "feature");

    // Run WITHOUT -vv
    repo.wt_command().args(["list"]).output().unwrap();

    // Diagnostic file should NOT exist
    let diagnostic_path = repo
        .root_path()
        .join(".git")
        .join("wt/logs")
        .join("diagnostic.md");
    assert!(
        !diagnostic_path.exists(),
        "Diagnostic file should NOT be created without -vv"
    );
}

/// Without -vv, the hint should tell users to re-run with -vv.
#[rstest]
fn test_diagnostic_hint_without_vv(mut repo: TestRepo) {
    repo.add_worktree("feature");
    corrupt_worktree_head(&repo, "feature");

    let output = repo.wt_command().args(["list"]).output().unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("To create a diagnostic file, run with"),
        "Should hint to use -vv. stderr: {}",
        stderr
    );
    assert!(
        stderr.contains("-vv"),
        "Hint should mention -vv flag. stderr: {}",
        stderr
    );
}

/// Diagnostic file must contain all required sections for GitHub issues.
#[rstest]
fn test_diagnostic_contains_required_sections(mut repo: TestRepo) {
    repo.add_worktree("feature");
    corrupt_worktree_head(&repo, "feature");

    repo.wt_command().args(["list", "-vv"]).output().unwrap();

    let content = fs::read_to_string(
        repo.root_path()
            .join(".git")
            .join("wt/logs")
            .join("diagnostic.md"),
    )
    .unwrap();

    // Header section
    assert!(
        content.contains("## Diagnostic Report"),
        "Should have header"
    );
    assert!(content.contains("**Generated:**"), "Should have timestamp");
    assert!(content.contains("**Command:**"), "Should have command");
    assert!(content.contains("**Result:**"), "Should have result");

    // Environment section
    assert!(
        content.contains("<summary>Environment</summary>"),
        "Should have environment section"
    );
    assert!(content.contains("wt "), "Should have wt version");
    assert!(content.contains("git "), "Should have git version");
    assert!(
        content.contains("Shell integration:"),
        "Should have shell integration status"
    );

    // Worktrees section
    assert!(
        content.contains("<summary>Worktrees</summary>"),
        "Should have worktrees section"
    );
    assert!(
        content.contains("refs/heads/"),
        "Should have branch refs in worktree list"
    );

    // Config section
    assert!(
        content.contains("<summary>Config</summary>"),
        "Should have config section"
    );

    // Trace log section
    assert!(
        content.contains("<summary>Trace log</summary>"),
        "Should have trace log section"
    );
}

/// The context field should have ANSI codes stripped for clean GitHub display.
#[rstest]
fn test_diagnostic_context_has_no_ansi_codes(mut repo: TestRepo) {
    repo.add_worktree("feature");
    corrupt_worktree_head(&repo, "feature");

    repo.wt_command().args(["list", "-vv"]).output().unwrap();

    let content = fs::read_to_string(
        repo.root_path()
            .join(".git")
            .join("wt/logs")
            .join("diagnostic.md"),
    )
    .unwrap();

    // ANSI escape codes start with \x1b[ or \033[
    assert!(
        !content.contains("\x1b["),
        "Diagnostic file should not contain ANSI escape codes"
    );
    assert!(
        !content.contains("\u{001b}"),
        "Diagnostic file should not contain ANSI escape codes (unicode)"
    );
}

/// Trace log should contain git command traces for debugging.
#[rstest]
fn test_diagnostic_trace_log_contains_git_commands(mut repo: TestRepo) {
    repo.add_worktree("feature");
    corrupt_worktree_head(&repo, "feature");

    repo.wt_command().args(["list", "-vv"]).output().unwrap();

    let content = fs::read_to_string(
        repo.root_path()
            .join(".git")
            .join("wt/logs")
            .join("diagnostic.md"),
    )
    .unwrap();

    // Extract trace log section
    let trace_start = content
        .find("<summary>Trace log</summary>")
        .expect("Should have trace log");
    let trace_section = &content[trace_start..];

    // Should contain git command traces
    assert!(
        trace_section.contains("git worktree list"),
        "Trace log should contain git worktree list command"
    );
    // Records are humanized: a ✓/✗ glyph reports each command's outcome with a
    // compact duration, replacing the old `[wt-trace] … dur_us=… ok=…` grammar.
    assert!(
        trace_section.contains("✓ git worktree list")
            || trace_section.contains("✗ git worktree list"),
        "Trace log should contain the humanized git worktree list completion record"
    );
    // In-process spans are humanized too (◷ glyph + duration).
    assert!(
        trace_section.contains("◷ init_logging"),
        "Trace log should contain the init_logging span record"
    );
}

/// Diagnostic is saved with -vv and output mentions it.
#[rstest]
fn test_diagnostic_saved_message_with_vv(mut repo: TestRepo) {
    repo.add_worktree("feature");
    corrupt_worktree_head(&repo, "feature");

    let output = repo.wt_command().args(["list", "-vv"]).output().unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);

    // Verify diagnostic was saved
    assert!(
        stderr.contains("Diagnostic saved"),
        "Should mention diagnostic was saved. stderr: {}",
        stderr
    );
}

/// Diagnostic file should be written to .git/wt/logs/diagnostic.md
#[rstest]
fn test_diagnostic_written_to_correct_location(mut repo: TestRepo) {
    repo.add_worktree("feature");
    corrupt_worktree_head(&repo, "feature");

    repo.wt_command().args(["list", "-vv"]).output().unwrap();

    // Should be in .git/wt/logs/ directory
    let wt_logs_dir = repo.root_path().join(".git").join("wt/logs");
    assert!(wt_logs_dir.exists());

    let diagnostic_path = wt_logs_dir.join("diagnostic.md");
    assert!(
        diagnostic_path.exists(),
        "diagnostic.md should be in wt/logs"
    );

    // Should be a markdown file
    let content = fs::read_to_string(&diagnostic_path).unwrap();
    assert!(
        content.starts_with("## "),
        "Should be a markdown file starting with header"
    );
}

/// Both log files (`trace.log` and `subprocess.log`) should be created at `-vv`.
#[rstest]
fn test_log_files_created(mut repo: TestRepo) {
    repo.add_worktree("feature");
    corrupt_worktree_head(&repo, "feature");

    repo.wt_command().args(["list", "-vv"]).output().unwrap();

    let logs_dir = repo.root_path().join(".git").join("wt/logs");
    let trace_log = logs_dir.join("trace.log");
    let subprocess_log = logs_dir.join("subprocess.log");
    assert!(trace_log.exists(), "trace.log should be created with -vv");
    assert!(
        subprocess_log.exists(),
        "subprocess.log should be created with -vv"
    );

    let trace = fs::read_to_string(&trace_log).unwrap();
    assert!(!trace.is_empty(), "trace.log should not be empty");
    assert!(
        trace.contains("◷ init_logging"),
        "trace.log should contain humanized trace records"
    );
}

/// At `-vv`, the full (uncapped) subprocess stdout/stderr should land in
/// `subprocess.log` via `shell_exec::SUBPROCESS_FULL_TARGET` — not in `trace.log`.
/// `trace.log` gets the bounded preview alongside trace records.
#[rstest]
fn test_vv_splits_full_and_bounded_output(repo: TestRepo) {
    repo.wt_command().args(["list", "-vv"]).output().unwrap();

    let logs_dir = repo.root_path().join(".git").join("wt/logs");
    let trace = fs::read_to_string(logs_dir.join("trace.log")).unwrap();
    let output = fs::read_to_string(logs_dir.join("subprocess.log")).unwrap();

    assert!(
        trace.contains("◷ init_logging"),
        "trace.log should contain humanized trace records at -vv"
    );
    // Captured stdout is emitted line-by-line with the `  ` / `  ! `
    // continuation prefix used by log_output. Full subprocess output lives
    // in subprocess.log.
    assert!(
        output.lines().any(|l| l.contains("  worktree ")),
        "subprocess.log should contain `git worktree list --porcelain` stdout lines at -vv"
    );
    // Trace records stay in trace.log only, not subprocess.log (which holds
    // raw bodies under their `$ cmd … [seq=N tid=T]` headers).
    assert!(
        !output.contains("◷ init_logging"),
        "subprocess.log should not contain trace records"
    );
}

/// Each command's block in `subprocess.log` is introduced by a
/// `$ cmd … [seq=N tid=T]` header, and that `seq` also tags the command's
/// record in the machine `trace.jsonl` — so an otherwise-undelimited raw output
/// block can be segmented and joined back to its timed command record. (The
/// humanized `trace.log` drops `seq`; the join lives in `trace.jsonl`.) Guards
/// the segmentation + correlation contract the files share.
#[rstest]
fn test_vv_subprocess_log_headers_join_trace(repo: TestRepo) {
    repo.wt_command().args(["list", "-vv"]).output().unwrap();

    let logs_dir = repo.root_path().join(".git").join("wt/logs");
    let subprocess = fs::read_to_string(logs_dir.join("subprocess.log")).unwrap();
    let trace = fs::read_to_string(logs_dir.join("trace.jsonl")).unwrap();

    // Pick a known captured command (`wt list` runs `git worktree list
    // --porcelain`) so the join can be checked by command identity, not just by
    // a seq value that — being process-global and dense — exists on *some*
    // record regardless. A header starts with `$ ` (body lines start with `  `,
    // so blocks self-segment) and carries the `seq` join key.
    let header = subprocess
        .lines()
        .find(|l| l.starts_with("$ ") && l.contains("git worktree list") && l.contains("[seq="))
        .expect("subprocess.log should head the `git worktree list` block with `$ … [seq=N …]`");

    let seq: u64 = header
        .split("[seq=")
        .nth(1)
        .and_then(|s| s.split([' ', ']']).next())
        .and_then(|s| s.parse().ok())
        .expect("the subprocess.log header should carry a numeric seq");

    // The trace.jsonl record carrying that seq must be the SAME command —
    // asserting both `seq` and `cmd` closes the gap where a dense seq coincides
    // with an unrelated record. (serde_json emits compact `"key":value`.)
    assert!(
        trace.lines().any(|l| {
            l.contains(&format!(r#""seq":{seq},"#)) && l.contains(r#""cmd":"git worktree list"#)
        }),
        "trace.jsonl should carry the record for `git worktree list` with seq={seq} to join the subprocess.log block:\n{header}"
    );
}

/// A command that pipes stdin has it captured in its `subprocess.log` block
/// under a `  < ` prefix, so the deep log shows what was fed in, not just what
/// came out. `wt list --full` counts each worktree's diff including untracked
/// files, staging them via `git add --pathspec-from-file=-` (stdin), which
/// exercises the path.
#[rstest]
fn test_vv_subprocess_log_captures_stdin(repo: TestRepo) {
    for path in repo.worktrees.values() {
        std::fs::write(path.join("untracked-probe.txt"), "stdin probe\n").unwrap();
    }
    repo.wt_command()
        .args(["list", "--full", "-vv"])
        .output()
        .unwrap();

    let subprocess =
        fs::read_to_string(repo.root_path().join(".git").join("wt/logs/subprocess.log")).unwrap();
    assert!(
        subprocess.lines().any(|l| l.starts_with("  < ")),
        "subprocess.log should capture piped stdin under a `  < ` prefix:\n{subprocess}"
    );
}

/// Control bytes in captured subprocess output must be escaped on the
/// human-facing routes (stderr + `trace.log`) but kept verbatim in
/// `subprocess.log`. `wt list` runs `git for-each-ref --format=…%00…`
/// (`LOCAL_BRANCH_FORMAT`), so its captured stdout carries NUL bytes between
/// fields — a real instance of the bytes that, left raw, make `diagnostic.md`
/// sniff as binary and break its `gh gist create` upload (issue #2988).
#[rstest]
fn test_vv_escapes_control_bytes_in_trace_not_subprocess(repo: TestRepo) {
    repo.wt_command().args(["list", "-vv"]).output().unwrap();

    let logs_dir = repo.root_path().join(".git").join("wt/logs");
    let trace = fs::read_to_string(logs_dir.join("trace.log")).unwrap();
    let subprocess = fs::read_to_string(logs_dir.join("subprocess.log")).unwrap();
    let diagnostic = fs::read_to_string(logs_dir.join("diagnostic.md")).unwrap();

    // subprocess.log is the raw byte stream — NUL survives verbatim.
    assert!(
        subprocess.contains('\u{0}'),
        "subprocess.log should keep raw NUL bytes from for-each-ref output"
    );
    // The bounded preview in trace.log is escaped: visible `\0`, no raw NUL.
    assert!(
        trace.contains(r"\0"),
        "trace.log should render NUL as the escaped `\\0`"
    );
    assert!(
        !trace.contains('\u{0}'),
        "trace.log must not carry raw NUL bytes (would sniff as binary)"
    );
    // diagnostic.md inlines trace.log plus config / worktree list — the upload
    // boundary must be free of raw control bytes so the gist upload succeeds.
    assert!(
        !diagnostic.contains('\u{0}'),
        "diagnostic.md must not carry raw NUL bytes (binary file → gist upload fails)"
    );
}

/// At `-vv`, Debug-level records (the noisy ones) stay out of stderr —
/// the bounded subprocess preview lands in `trace.log` (not stderr), and
/// `subprocess.log` still holds the unbounded body. Info-level routing
/// from `-v` still applies at `-vv` (it's a superset), so the "Writing
/// to ..." pointer and similar status lines DO appear on stderr; they're
/// asserted at the end of this test. Guards against a regression that
/// re-routes the debug stream to stderr and floods the terminal.
#[rstest]
fn test_vv_debug_pipeline_silent_on_stderr(repo: TestRepo) {
    // `wt list` runs `git for-each-ref refs/heads/` (captured via `Cmd::run`),
    // so its output flows through `log_output` and exercises the split.
    // Populate packed-refs with 250 fake branches pointing at HEAD — far more
    // than LOG_OUTPUT_MAX_LINES (200), which would have tripped elision on
    // stderr under the old routing.
    let head_out = repo
        .git_command()
        .args(["rev-parse", "HEAD"])
        .run()
        .unwrap();
    let head = String::from_utf8(head_out.stdout)
        .unwrap()
        .trim()
        .to_string();
    let mut content = String::from("# pack-refs with: peeled fully-peeled sorted\n");
    for i in 0..250 {
        use std::fmt::Write as _;
        writeln!(&mut content, "{head} refs/heads/many-branch-{i:03}").unwrap();
    }
    std::fs::write(repo.root_path().join(".git/packed-refs"), content).unwrap();

    let output = repo
        .wt_command()
        .args(["list", "-vv"])
        .env("NO_COLOR", "1")
        .output()
        .expect("wt list");
    let stderr = String::from_utf8_lossy(&output.stderr);

    let logs_dir = repo.root_path().join(".git").join("wt/logs");
    let trace = fs::read_to_string(logs_dir.join("trace.log")).unwrap();
    let raw = fs::read_to_string(logs_dir.join("subprocess.log")).unwrap();

    // Elision marker belongs to the bounded preview, which now lives in
    // trace.log only — never on stderr.
    let marker = "more lines, ";
    assert!(
        trace.contains(marker),
        "trace.log should hold the bounded preview's elision marker"
    );
    assert!(
        !stderr.contains(marker),
        "stderr must not receive the bounded preview at -vv: {stderr}"
    );
    assert!(
        !raw.contains(marker),
        "subprocess.log holds full output and must not contain an elision marker"
    );

    // The tail refs only appear in the full log.
    let tail_ref = "many-branch-249";
    assert!(
        raw.contains(tail_ref),
        "subprocess.log should contain the full for-each-ref stdout (last ref)"
    );
    assert!(
        !stderr.contains(tail_ref),
        "stderr must not see the per-line subprocess output at -vv"
    );
    assert!(
        !trace.contains(tail_ref),
        "trace.log holds the bounded preview, which is capped before the last ref"
    );

    // The full subprocess stdout still lands in subprocess.log, and trace.log
    // still captures the humanized trace records — confirm both so a
    // regression that disables the file sinks fails loudly here too.
    assert!(
        raw.lines().any(|l| l.contains("refs/heads/many-branch-")),
        "subprocess.log should contain the captured for-each-ref stdout"
    );
    assert!(
        trace.contains("◷ init_logging"),
        "trace.log should contain humanized trace records at -vv"
    );

    // Stderr at -vv should announce each file's full path on its own line, so
    // any one is copy-pasteable without joining a shared root to a filename.
    // The `wt/logs/<file>` tails only appear contiguously when the full path
    // is listed (the old root-plus-filenames form split them apart).
    assert!(
        stderr.contains("Writing to"),
        "missing pointer header: {stderr}"
    );
    for file in ["trace.log", "subprocess.log", "diagnostic.md"] {
        assert!(
            stderr.contains(&format!("wt/logs/{file}")),
            "stderr should list the full path to {file}: {stderr}"
        );
    }
}

/// `RUST_LOG` is honored at every verbosity level — including `-v` — and
/// can raise the filter above the flag's baseline. A user who runs
/// `RUST_LOG=debug wt -v` gets Debug, not Info: the flag sets a baseline,
/// and `RUST_LOG` refines it on top via `tracing_subscriber::EnvFilter`'s
/// directive grammar (env wins when set). Guards against the regression
/// where `-v`/`-vv` hardcoded `filter_level(...)` and silently dropped
/// `RUST_LOG`.
///
/// The probe is the `◷ init_logging` span record — emitted at `log::debug!`,
/// unique to the trace stream (so it can't be confused with normal output),
/// and present on every run. It's suppressed at the Info baseline `-v` selects
/// and surfaces when `RUST_LOG=debug` raises it. (At `-v 0` the same
/// `RUST_LOG=debug` path is already covered by
/// `test_rust_log_debug_fallback_without_vv`; at `-vv` the baseline is
/// already Debug so there's no flag/env conflict to assert.)
#[rstest]
fn test_rust_log_overrides_verbose_flag(repo: TestRepo) {
    // Baseline: -v alone caps at Info, so debug-level trace records are
    // dropped from stderr.
    let info_only = repo
        .wt_command()
        .args(["list", "-v"])
        .env_remove("RUST_LOG")
        .env("NO_COLOR", "1")
        .output()
        .expect("wt list -v");
    let info_stderr = String::from_utf8_lossy(&info_only.stderr);
    assert!(
        !info_stderr.contains("init_logging"),
        "-v alone (Info baseline) should not surface trace records: {info_stderr}"
    );

    // RUST_LOG=debug + -v raises the level: trace records appear.
    let with_env = repo
        .wt_command()
        .args(["list", "-v"])
        .env("RUST_LOG", "debug")
        .env("NO_COLOR", "1")
        .output()
        .expect("wt list -v");
    let env_stderr = String::from_utf8_lossy(&with_env.stderr);
    assert!(
        env_stderr.contains("init_logging"),
        "RUST_LOG=debug at -v should surface trace records on stderr: {env_stderr}"
    );
}

/// `RUST_LOG=debug` at `-v 0` activates Debug logging without creating the
/// on-disk log files. Stderr receives the **bounded** preview
/// (`SUBPROCESS_BOUNDED_TARGET`); the uncapped `SUBPROCESS_FULL_TARGET`
/// records are dropped so raw bodies don't flood the terminal.
#[rstest]
fn test_rust_log_debug_fallback_without_vv(repo: TestRepo) {
    let output = repo
        .wt_command()
        .args(["list"])
        .env("RUST_LOG", "debug")
        .env("NO_COLOR", "1")
        .output()
        .expect("wt list");
    let stderr = String::from_utf8_lossy(&output.stderr);

    // No log files were created — `-vv` is what opens them.
    let logs_dir = repo.root_path().join(".git").join("wt/logs");
    assert!(
        !logs_dir.join("trace.log").exists(),
        "trace.log should NOT be created without -vv"
    );
    assert!(
        !logs_dir.join("subprocess.log").exists(),
        "subprocess.log should NOT be created without -vv"
    );

    // Subprocess stdout still reaches stderr via the bounded preview —
    // look for captured `git worktree list --porcelain` stdout lines.
    assert!(
        stderr.lines().any(|l| l.contains("  worktree ")),
        "stderr should contain subprocess stdout via bounded preview: {stderr}"
    );
    // For small outputs (git worktree list --porcelain is a handful of lines)
    // the bounded preview fits under the cap without an elision marker.
    assert!(
        !stderr.contains("more lines, "),
        "short subprocess output should not trip the elision marker: {stderr}"
    );
}

/// `WORKTRUNK_VERBOSE` sets the verbosity level from the environment, mirroring
/// the `-v`/`-vv` flags so logging can be turned on with no CLI flag — the only
/// way to reach shell completion, which has nowhere to pass `-vv` (see the
/// completion suite). The env and the flag combine via `max`: the env is a
/// baseline the flags raise but never lower. Level 2 opens the on-disk trace
/// files exactly like `-vv`; level 1 does not, proving the count is threaded
/// rather than collapsed to a boolean "verbose on".
#[rstest]
fn test_worktrunk_verbose_env_sets_level(repo: TestRepo) {
    let logs_dir = repo.root_path().join(".git").join("wt/logs");
    let trace_log = logs_dir.join("trace.log");

    // Level 2 with no flag opens the trace files, just like `-vv`.
    let _ = fs::remove_dir_all(&logs_dir);
    let out = repo
        .wt_command()
        .args(["list"])
        .env("WORKTRUNK_VERBOSE", "2")
        .env("NO_COLOR", "1")
        .output()
        .expect("wt list");
    assert!(out.status.success());
    assert!(
        trace_log.exists(),
        "WORKTRUNK_VERBOSE=2 should open trace.log like -vv"
    );

    // Level 1 with no flag does NOT — the trace files are a level-2 artifact,
    // so the env must carry the count, not just a boolean.
    let _ = fs::remove_dir_all(&logs_dir);
    let out = repo
        .wt_command()
        .args(["list"])
        .env("WORKTRUNK_VERBOSE", "1")
        .env("NO_COLOR", "1")
        .output()
        .expect("wt list");
    assert!(out.status.success());
    assert!(
        !trace_log.exists(),
        "WORKTRUNK_VERBOSE=1 should not open the -vv trace files"
    );

    // An explicit `-vv` wins over a lower env baseline (`max`, not env-wins):
    // env 0 + `-vv` still writes the files.
    let _ = fs::remove_dir_all(&logs_dir);
    let out = repo
        .wt_command()
        .args(["list", "-vv"])
        .env("WORKTRUNK_VERBOSE", "0")
        .env("NO_COLOR", "1")
        .output()
        .expect("wt list -vv");
    assert!(out.status.success());
    assert!(
        trace_log.exists(),
        "an explicit -vv must win over WORKTRUNK_VERBOSE=0"
    );
}

// =============================================================================
// Tests for -vv verbosity level (always write diagnostic)
// =============================================================================

/// With -vv, diagnostic file should be written even on successful commands.
#[rstest]
fn test_vv_writes_diagnostic_on_success(repo: TestRepo) {
    // Run a successful command with -vv
    let output = repo.wt_command().args(["list", "-vv"]).output().unwrap();

    assert!(output.status.success(), "Command should succeed");

    // Diagnostic file should exist
    let diagnostic_path = repo
        .root_path()
        .join(".git")
        .join("wt/logs")
        .join("diagnostic.md");
    assert!(
        diagnostic_path.exists(),
        "Diagnostic file should be created with -vv even on success"
    );

    // Content should indicate success
    let content = fs::read_to_string(&diagnostic_path).unwrap();
    assert!(
        content.contains("Command completed successfully"),
        "Result should indicate success. Content: {}",
        content
    );

    // stderr should mention diagnostic was saved
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Diagnostic saved"),
        "stderr should mention diagnostic was saved. stderr: {}",
        stderr
    );
}

/// With -vv, diagnostic file should be written on error (same as success case).
#[rstest]
fn test_vv_writes_diagnostic_on_error(mut repo: TestRepo) {
    repo.add_worktree("feature");
    corrupt_worktree_head(&repo, "feature");

    // Run a command that will hit git errors with -vv
    let output = repo.wt_command().args(["list", "-vv"]).output().unwrap();

    // Diagnostic file should exist
    let diagnostic_path = repo
        .root_path()
        .join(".git")
        .join("wt/logs")
        .join("diagnostic.md");
    assert!(
        diagnostic_path.exists(),
        "Diagnostic file should be created with -vv on error"
    );

    // stderr should mention diagnostic was saved
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Diagnostic saved"),
        "stderr should mention diagnostic was saved. stderr: {}",
        stderr
    );
}

/// If one of the `-vv` log files can't be opened (here: pre-existing path
/// of the wrong type), the user still gets a "Writing to ..." pointer for
/// the one that opened, and the elision marker reflects the asymmetric
/// state instead of telling a `-vv` user to "rerun with -vv".
#[rstest]
fn test_vv_pointer_handles_split_init(repo: TestRepo) {
    let logs_dir = repo.root_path().join(".git").join("wt/logs");
    std::fs::create_dir_all(&logs_dir).unwrap();
    // Block subprocess.log open by occupying the path with a directory.
    std::fs::create_dir(logs_dir.join("subprocess.log")).unwrap();

    let output = repo
        .wt_command()
        .args(["list", "-vv"])
        .env("NO_COLOR", "1")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        stderr.contains("Writing to") && stderr.contains("wt/logs/trace.log"),
        "stderr should list trace.log's full path even when subprocess.log can't open: {stderr}"
    );
    assert!(
        stderr.contains("subprocess.log unavailable"),
        "stderr should note subprocess.log is unavailable: {stderr}"
    );
    // subprocess.log didn't open, so it's named only in the unavailable note —
    // never listed as a live path in the gutter.
    assert!(
        !stderr.contains("wt/logs/subprocess.log"),
        "stderr should not list a path for the unavailable subprocess.log: {stderr}"
    );
    assert!(
        logs_dir.join("trace.log").exists(),
        "trace.log should still be created"
    );
}

/// With just -v, info-level logging goes to stderr but no log files are written.
/// `-vv` is the threshold for `trace.log`, `subprocess.log`, and `diagnostic.md`.
#[rstest]
fn test_v_does_not_write_log_files(repo: TestRepo) {
    // Run a successful command with just -v
    let output = repo.wt_command().args(["list", "-v"]).output().unwrap();

    assert!(output.status.success(), "Command should succeed");

    // None of the -vv diagnostic files should exist with just -v
    let wt_logs = repo.root_path().join(".git").join("wt/logs");
    for name in ["diagnostic.md", "trace.log", "subprocess.log"] {
        assert!(
            !wt_logs.join(name).exists(),
            "{name} should NOT be created with just -v (requires -vv)"
        );
    }
}

/// With -vv outside a git repo, command should still work (no crash), no
/// log files are written, and `announce_trace_destination` takes its silent
/// early-return path (TRACE.path() is None).
#[test]
fn test_vv_outside_repo_no_crash() {
    use crate::common::wt_command;

    // Create a temp directory that is NOT a git repo
    let temp_dir = tempfile::tempdir().unwrap();

    // Use `list` (not `--version`) so `init_logging` actually runs — clap
    // short-circuits `--version` before logging is set up, which would
    // skip the announce_trace_destination path this test exists to cover.
    let output = wt_command()
        .args(["list", "-vv"])
        .current_dir(temp_dir.path())
        .output()
        .unwrap();

    // `wt list` exits non-zero outside a git repo, but that's fine —
    // init_logging still ran before the command failed.
    assert!(
        !output.status.success(),
        "wt list outside a repo should fail"
    );

    // No diagnostic file should be created (not in a git repo)
    let diagnostic_path = temp_dir
        .path()
        .join(".git")
        .join("wt/logs")
        .join("diagnostic.md");
    assert!(
        !diagnostic_path.exists(),
        "Diagnostic file should NOT be created outside a git repo"
    );

    // Startup pointer should NOT appear: TRACE failed to open, so
    // announce_trace_destination took its early-return path.
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("Writing to"),
        "no startup pointer when trace.log can't open: {stderr}"
    );
}

/// When gh is installed, the hint should show gist creation and issue URL.
#[rstest]
fn test_diagnostic_gh_hint_with_vv(mut repo: TestRepo) {
    // Setup mock gh so it appears installed
    repo.setup_mock_gh();

    let output = repo.wt_command().args(["list", "-vv"]).output().unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);

    // Extract the hint line (starts with ↳)
    let hint_line = stderr
        .lines()
        .find(|line| line.contains("report a bug"))
        .expect("Should have hint about reporting a bug");

    // Normalize the path in the hint. The path may be:
    // - Quoted on Windows (drive letter colon requires POSIX escaping): 'D:/a/.../diagnostic.md'
    // - Unquoted on Unix (no special chars): _REPO_/.git/wt/logs/diagnostic.md
    let normalized = regex::Regex::new(r"--web '?[^' \x1b]*diagnostic\.md'?")
        .unwrap()
        .replace(hint_line, "--web [DIAGNOSTIC_PATH]");

    let settings = setup_snapshot_settings(&repo);
    settings.bind(|| {
        assert_snapshot!("diagnostic_gh_hint", normalized);
    });
}

/// Normalize the report for snapshot comparison.
///
/// Replaces variable content (versions, paths, timestamps) with placeholders.
fn normalize_report(content: &str) -> String {
    let mut result = content.to_string();

    // Normalize timestamp (e.g., "2025-01-01T00:00:00Z" -> "[TIMESTAMP]")
    result = regex::Regex::new(r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z")
        .unwrap()
        .replace_all(&result, "[TIMESTAMP]")
        .to_string();

    // Normalize command line path (binary path varies between environments)
    // Matches: `target/debug/wt ...` or `/path/to/wt ...`
    result = regex::Regex::new(r"\*\*Command:\*\* `[^`]+`")
        .unwrap()
        .replace_all(
            &result,
            "**Command:** `[PROJECT_ROOT]/target/[BUILD_MODE]/wt list -vv`",
        )
        .to_string();

    // Normalize wt version line (e.g., "wt v0.9.3-dirty (macos aarch64)" or "wt be89089 (linux x86_64)")
    // CI builds use commit hashes instead of version numbers
    result = regex::Regex::new(r"wt [^ ]+ \([^)]+\)")
        .unwrap()
        .replace_all(&result, "wt [VERSION] ([OS] [ARCH])")
        .to_string();

    // Normalize git version line
    result = regex::Regex::new(r"git \d+\.\d+[^\n]*")
        .unwrap()
        .replace_all(&result, "git [GIT_VERSION]")
        .to_string();

    // Normalize worktree paths in porcelain output (Unix and Windows absolute paths)
    result = regex::Regex::new(r"worktree (?:/|[A-Za-z]:)[^\n]+")
        .unwrap()
        .replace_all(&result, "worktree [PATH]")
        .to_string();

    // Normalize commit hashes (40 hex chars) - in "HEAD xxx" format
    result = regex::Regex::new(r"HEAD [a-f0-9]{40}")
        .unwrap()
        .replace_all(&result, "HEAD [COMMIT]")
        .to_string();

    // Normalize all other commit hashes (40 hex chars standalone)
    result = regex::Regex::new(r"\b[a-f0-9]{40}\b")
        .unwrap()
        .replace_all(&result, "[HASH]")
        .to_string();

    // Normalize user config path (must come BEFORE generic repo path normalization)
    result = regex::Regex::new(r"User config: [^\n]+")
        .unwrap()
        .replace_all(&result, "User config: [TEST_CONFIG]")
        .to_string();

    // Normalize project config path (must come BEFORE generic repo path normalization)
    // Handle both Unix (/) and Windows (\) path separators
    result = regex::Regex::new(r"Project config: (?:/|[A-Za-z]:)[^\n]+\.config[/\\]wt\.toml")
        .unwrap()
        .replace_all(&result, "Project config: _REPO_/.config/wt.toml")
        .to_string();

    // Normalize worktree temp paths (paths that contain `/repo.<name>` or
    // `\repo.<name>` — the convention used by TestRepo for linked worktrees).
    // Both branches require the `repo.` segment so the main-worktree path
    // (which is `.../repo` without a dot) falls through to insta's prefix
    // filter instead. Stop at whitespace, `)`, or a backtick so paths inlined
    // in markdown code spans don't eat the closing backtick.
    result = regex::Regex::new(r"([A-Z]:[^\s)`]+[\\/]repo\.[^\s)`]+|/[^\s)`]+/repo\.[^\s)`]+)")
        .unwrap()
        .replace_all(&result, "[REPO_PATH]")
        .to_string();

    // Normalize line breaks in git error messages (cross-platform consistency)
    // Some platforms wrap "fatal: not a git repository:\n  /path" on two lines,
    // others keep it on one line. Normalize to single-line format.
    result = regex::Regex::new(r"(fatal: not a git repository:)\s*\n\s*(\[REPO_PATH\])")
        .unwrap()
        .replace_all(&result, "$1 $2")
        .to_string();

    // Truncate performance profile section - its durations and by-type/slowest
    // ordering depend on the run's real timing, so exact comparison is flaky.
    // We verify the section exists separately in the test.
    if let Some(start) = result.find("<summary>Performance profile</summary>") {
        // Find the closing </details> after this point
        if let Some(end_offset) = result[start..].find("</details>") {
            let end = start + end_offset + "</details>".len();
            let before = &result[..start];
            let after = &result[end..];
            result = format!(
                "{}<summary>Performance profile</summary>\n\n[PERFORMANCE_PROFILE_CONTENT]\n</details>{}",
                before, after
            );
        }
    }

    // Truncate trace log section - it has parallel git commands that interleave
    // in different orders, making exact snapshot comparison flaky.
    // We verify the section exists separately in the test.
    if let Some(start) = result.find("<summary>Trace log</summary>") {
        // Find the closing </details> after this point
        if let Some(end_offset) = result[start..].find("</details>") {
            let end = start + end_offset + "</details>".len();
            let before = &result[..start];
            let after = &result[end..];
            result = format!(
                "{}<summary>Trace log</summary>\n\n[TRACE_LOG_CONTENT]\n</details>{}",
                before, after
            );
        }
    }

    result
}
