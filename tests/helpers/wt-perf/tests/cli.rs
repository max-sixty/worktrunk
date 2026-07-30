//! Integration tests for the wt-perf trace command.
//!
//! In-package so cargo builds the wt-perf binary to run them and provides its
//! path via `CARGO_BIN_EXE_wt-perf` — this is what triggers the binary build
//! under a workspace `cargo test` (see Cargo.toml's header comment).

use std::io::Write;
use std::process::{Command, Stdio};

fn wt_perf_bin() -> &'static str {
    env!("CARGO_BIN_EXE_wt-perf")
}

#[test]
fn setup_help_exposes_the_semantic_fixture_catalog() {
    let output = Command::new(wt_perf_bin())
        .args(["setup", "--help"])
        .output()
        .expect("Failed to run wt-perf setup --help");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);

    for subcommand in [
        "typical",
        "minimal",
        "synthetic-divergence",
        "large-repository-worktrees",
        "large-repository-history-spread",
        "mixed",
        "prune",
        "picker-test",
    ] {
        assert!(
            stdout.contains(subcommand),
            "setup help must list {subcommand}:\n{stdout}"
        );
    }
}

#[test]
fn setup_subcommands_parse_without_acquiring_large_repository_fixtures() {
    for subcommand in [
        "typical",
        "minimal",
        "synthetic-divergence",
        "large-repository-worktrees",
        "large-repository-history-spread",
        "mixed",
        "prune",
        "picker-test",
    ] {
        let output = Command::new(wt_perf_bin())
            .args(["setup", subcommand, "--help"])
            .output()
            .unwrap_or_else(|error| panic!("Failed to parse setup {subcommand}: {error}"));
        assert!(
            output.status.success(),
            "setup {subcommand} --help failed:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let path = tempfile::tempdir().expect("Failed to create temp dir");
    let output = Command::new(wt_perf_bin())
        .args([
            "setup",
            "prune",
            "--base",
            "large-repository",
            "--path",
            path.path().to_str().unwrap(),
        ])
        .output()
        .expect("Failed to parse managed prune setup");
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("large-repository prune fixtures are managed")
    );

    let output = Command::new(wt_perf_bin())
        .args(["setup", "large-repository-history-spread", "5001"])
        .output()
        .expect("Failed to parse unsupported history-spread count");
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("expected at most 5000 branches"),
        "unsupported count must fail before acquiring the large repository:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn setup_builds_a_semantic_recipe_at_an_explicit_path() {
    let root = tempfile::tempdir().expect("Failed to create temp dir");
    let repo = root.path().join("minimal-fixture");
    let output = Command::new(wt_perf_bin())
        .args([
            "setup",
            "minimal",
            "0",
            "0",
            "--path",
            repo.to_str().unwrap(),
        ])
        .output()
        .expect("Failed to build minimal fixture");
    assert!(
        output.status.success(),
        "setup failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(repo.join(".git").is_dir(), "setup must create a git repo");

    let branches = Command::new("git")
        .args([
            "-C",
            repo.to_str().unwrap(),
            "branch",
            "--format=%(refname:short)",
        ])
        .output()
        .expect("Failed to inspect built fixture");
    assert!(branches.status.success());
    assert_eq!(String::from_utf8_lossy(&branches.stdout), "main\n");
}

#[test]
fn setup_default_path_rebuilds_a_linked_worktree_fixture() {
    // Run a copied binary so its executable-derived target directory lives
    // under the test tempdir rather than touching the workspace's fixtures.
    let target = tempfile::tempdir().expect("Failed to create isolated target dir");
    let debug = target.path().join("debug");
    std::fs::create_dir(&debug).expect("Failed to create isolated debug dir");
    let binary = debug.join(
        std::path::Path::new(wt_perf_bin())
            .file_name()
            .expect("wt-perf binary must have a file name"),
    );
    std::fs::copy(wt_perf_bin(), &binary).expect("Failed to copy wt-perf binary");
    let unrelated = target.path().join("wt-perf/minimal-0-1.unrelated");
    std::fs::create_dir_all(&unrelated).expect("Failed to create unrelated sibling");
    std::fs::write(unrelated.join("sentinel"), "keep").expect("Failed to write sentinel");

    for run in 1..=2 {
        let output = Command::new(&binary)
            .args(["setup", "minimal", "0", "1"])
            .output()
            .unwrap_or_else(|error| panic!("Failed to run setup attempt {run}: {error}"));
        assert!(
            output.status.success(),
            "setup attempt {run} failed:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    assert_eq!(
        std::fs::read_to_string(unrelated.join("sentinel")).unwrap(),
        "keep",
        "setup must remove exact registered worktrees, not unrelated siblings"
    );
}

/// Test that the binary produces Chrome Trace Format JSON for sample trace input.
#[test]
fn test_wt_perf_trace_from_stdin() {
    let sample_trace = r#"{"kind":"cmd_completed","ts":1000000,"tid":1,"cmd":"git status","dur_us":10000,"ok":true}
{"kind":"cmd_completed","ts":1010000,"tid":1,"cmd":"git status","dur_us":15000,"ok":true}
{"kind":"cmd_completed","ts":1025000,"tid":1,"cmd":"git diff","dur_us":100000,"ok":true}
{"kind":"instant","ts":1025000,"tid":2,"event":"Showed skeleton"}
{"kind":"cmd_completed","ts":1125000,"tid":1,"cmd":"git merge-base HEAD main","dur_us":500000,"ok":true}
{"kind":"cmd_completed","ts":1625000,"tid":1,"cmd":"gh pr list","dur_us":200000,"ok":true}"#;

    let mut child = Command::new(wt_perf_bin())
        .arg("trace")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to spawn wt-perf");

    child
        .stdin
        .take()
        .unwrap()
        .write_all(sample_trace.as_bytes())
        .expect("Failed to write to stdin");

    let output = child.wait_with_output().expect("Failed to read output");

    assert!(output.status.success(), "wt-perf trace should succeed");

    let stdout = String::from_utf8_lossy(&output.stdout);

    // Verify it's valid Chrome Trace Format JSON
    let json: serde_json::Value = serde_json::from_str(&stdout).expect("Should output valid JSON");

    assert_eq!(json["displayTimeUnit"], "ms", "Should have displayTimeUnit");
    assert!(
        json["traceEvents"].is_array(),
        "Should have traceEvents array"
    );

    let events = json["traceEvents"].as_array().unwrap();
    assert_eq!(events.len(), 6, "Should have 6 events");

    // Check command events
    assert_eq!(events[0]["name"], "git status");
    assert_eq!(events[0]["ph"], "X"); // Complete event
    assert!(events[0]["dur"].is_number()); // Has duration

    // Check instant event
    assert_eq!(events[3]["name"], "Showed skeleton");
    assert_eq!(events[3]["ph"], "I"); // Instant event
    assert_eq!(events[3]["s"], "g"); // Global scope
    assert!(events[3]["dur"].is_null()); // No duration
}

/// Test that the binary shows usage when run interactively without input.
#[test]
fn test_wt_perf_trace_no_input_shows_usage() {
    // Test by passing a non-existent file
    let output = Command::new(wt_perf_bin())
        .args(["trace", "/nonexistent/path/to/file.log"])
        .output()
        .expect("Failed to run wt-perf");

    assert!(
        !output.status.success(),
        "Should fail with non-existent file"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Error reading"),
        "Should show error message"
    );
}

/// Test that the binary handles empty trace input.
#[test]
fn test_wt_perf_trace_empty_input() {
    let mut child = Command::new(wt_perf_bin())
        .arg("trace")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to spawn wt-perf");

    // Write empty input and close stdin
    child.stdin.take().unwrap();

    let output = child.wait_with_output().expect("Failed to read output");

    assert!(
        !output.status.success(),
        "Should fail with no trace entries"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("No trace records found"),
        "Should indicate no trace records"
    );
}

/// Test reading from a file.
#[test]
fn test_wt_perf_trace_from_file() {
    // Create a temp file with sample trace data
    let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
    let log_file = temp_dir.path().join("trace.jsonl");

    let sample_trace = r#"{"kind":"cmd_completed","ts":1000000,"tid":1,"cmd":"git rev-parse","dur_us":5000,"ok":true}
{"kind":"cmd_completed","ts":1005000,"tid":1,"cmd":"git status","dur_us":10000,"ok":true}
{"kind":"instant","ts":1015000,"tid":1,"event":"Skeleton displayed"}
{"kind":"cmd_completed","ts":1015000,"tid":2,"cmd":"git diff","dur_us":50000,"ok":true}"#;

    std::fs::write(&log_file, sample_trace).expect("Failed to write sample log");

    let output = Command::new(wt_perf_bin())
        .args(["trace", log_file.to_str().unwrap()])
        .output()
        .expect("Failed to run wt-perf");

    assert!(output.status.success(), "Should succeed with sample log");

    let stdout = String::from_utf8_lossy(&output.stdout);

    // Verify it's valid Chrome Trace Format JSON
    let json: serde_json::Value = serde_json::from_str(&stdout).expect("Should output valid JSON");

    assert!(json["traceEvents"].is_array(), "Should have traceEvents");
    let events = json["traceEvents"].as_array().unwrap();
    assert_eq!(events.len(), 4, "Should have 4 events");

    // Check we have both command and instant events
    assert_eq!(events[0]["name"], "git rev-parse");
    assert_eq!(events[2]["name"], "Skeleton displayed");
    assert_eq!(events[2]["ph"], "I"); // Instant event
}
