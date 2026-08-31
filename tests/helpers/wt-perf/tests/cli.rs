//! Integration tests for the wt-perf CLI.
//!
//! In-package so cargo builds the wt-perf binary to run them and provides its
//! path via `CARGO_BIN_EXE_wt-perf` — this is what triggers the binary build
//! under a workspace `cargo test` (see Cargo.toml's header comment).

use std::io::Write;
use std::process::{Command, Output, Stdio};

use worktrunk::testing::configure_git_cmd;

fn wt_perf_bin() -> &'static str {
    env!("CARGO_BIN_EXE_wt-perf")
}

fn run_wt_perf(args: &[&str]) -> Output {
    Command::new(wt_perf_bin())
        .args(args)
        .output()
        .expect("Failed to run wt-perf")
}

#[test]
fn help_exposes_only_canonical_operations() {
    let output = run_wt_perf(&["--help"]);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);

    for subcommand in ["setup", "trace", "timeline"] {
        assert!(
            stdout.contains(subcommand),
            "help must list {subcommand}:\n{stdout}"
        );
    }
    assert!(
        !stdout.contains("invalidate"),
        "cold-cache investigation must route through timeline --cold:\n{stdout}"
    );
}

#[test]
fn setup_help_exposes_the_semantic_fixture_catalog() {
    let output = run_wt_perf(&["setup", "--help"]);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("--path <PATH>"),
        "setup must require an explicit destination:\n{stdout}"
    );

    let commands = stdout
        .split("Commands:\n")
        .nth(1)
        .unwrap()
        .split("\n\nOptions:")
        .next()
        .unwrap()
        .lines()
        .filter_map(|line| {
            line.strip_prefix("  ")
                .filter(|line| !line.starts_with(' '))
        })
        .filter_map(|line| line.split_whitespace().next())
        .collect::<Vec<_>>();
    assert_eq!(commands, ["generated", "imported", "help"]);
    for overlay in ["--prune-candidates", "--prune-backdrop"] {
        assert!(
            stdout.contains(overlay),
            "setup help must expose prune state as an overlay:\n{stdout}"
        );
    }
}

#[test]
fn setup_validation_does_not_acquire_imported_fixture() {
    let output = run_wt_perf(&["setup", "generated"]);
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("Missing required --path"),
        "setup must enforce an explicit destination:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let path = tempfile::tempdir().expect("Failed to create temp dir");
    let output = run_wt_perf(&[
        "setup",
        "imported",
        "--prune-candidates",
        "12",
        "--prune-backdrop",
        "24",
        "--path",
        path.path().to_str().unwrap(),
    ]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("Destination already exists"));
}

#[test]
fn setup_builds_a_semantic_recipe_at_an_explicit_path() {
    let root = tempfile::tempdir().expect("Failed to create temp dir");
    let repo = root.path().join("generated-fixture");
    let output = run_wt_perf(&["setup", "generated", "--path", repo.to_str().unwrap()]);
    assert!(
        output.status.success(),
        "setup failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("wt-perf timeline --cold -- -C"),
        "setup must direct cold investigation through timeline --cold:\n{stderr}"
    );
    assert!(
        !stderr.contains("wt-perf invalidate"),
        "setup must not advertise the removed standalone command:\n{stderr}"
    );
    assert!(repo.join(".git").is_dir(), "setup must create a git repo");
}

#[test]
fn setup_layers_prune_state_onto_a_canonical_base() {
    let root = tempfile::tempdir().expect("Failed to create temp dir");
    let repo = root.path().join("prune-fixture");
    let output = run_wt_perf(&[
        "setup",
        "generated",
        "--prune-candidates",
        "1",
        "--prune-backdrop",
        "1",
        "--path",
        repo.to_str().unwrap(),
    ]);
    assert!(
        output.status.success(),
        "setup failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Fixture inspection is still a `git` the suite runs, so it gets the same
    // isolation as every other one — the crate's own `git_command()` helper is
    // private to the lib, so apply the floor directly here.
    let mut git = Command::new("git");
    configure_git_cmd(&mut git);
    let branches = git
        .args([
            "-C",
            repo.to_str().unwrap(),
            "branch",
            "--format=%(refname:short)",
        ])
        .output()
        .expect("Failed to inspect built fixture");
    assert!(branches.status.success());
    let branches = String::from_utf8(branches.stdout).unwrap();
    for branch in [
        "main",
        "merged-br-0",
        "merged-wt-0",
        "unmerged-br-000",
        "unmerged-wt-0",
    ] {
        assert!(
            branches.lines().any(|found| found == branch),
            "missing {branch}:\n{branches}"
        );
    }
}

#[test]
fn setup_never_mutates_an_existing_destination() {
    let root = tempfile::tempdir().expect("Failed to create temp dir");
    let destination = root.path().join("existing");
    std::fs::create_dir(&destination).expect("Failed to create destination");
    let sentinel = destination.join("sentinel");
    std::fs::write(&sentinel, "keep").expect("Failed to write sentinel");

    for path in [
        destination.clone(),
        root.path().join("missing-parent/../existing"),
    ] {
        let output = run_wt_perf(&["setup", "generated", "--path", path.to_str().unwrap()]);
        assert!(
            !output.status.success(),
            "setup unexpectedly accepted {path:?}"
        );
        assert_eq!(std::fs::read_to_string(&sentinel).unwrap(), "keep");
        assert!(!destination.join(".git").exists());
    }
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
    let output = run_wt_perf(&["trace", "/nonexistent/path/to/file.log"]);

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
