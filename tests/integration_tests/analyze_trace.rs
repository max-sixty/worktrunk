//! Integration tests for the wt-perf trace command.

use std::io::Write;
use std::process::{Command, Stdio};

fn wt_perf_bin() -> std::path::PathBuf {
    // Build and get path to wt-perf binary
    let output = Command::new("cargo")
        .args(["build", "-p", "wt-perf"])
        .output()
        .expect("Failed to build wt-perf");
    assert!(output.status.success(), "Failed to build wt-perf");

    std::env::current_dir()
        .unwrap()
        .join("target/debug/wt-perf")
}

/// Test that the binary produces Chrome Trace Format JSON for sample trace input.
#[test]
fn test_wt_perf_trace_from_stdin() {
    let sample_trace = r#"[wt-trace] ts=1000000 tid=1 cmd="git status" dur=10.0ms ok=true
[wt-trace] ts=1010000 tid=1 cmd="git status" dur=15.0ms ok=true
[wt-trace] ts=1025000 tid=1 cmd="git diff" dur=100.0ms ok=true
[wt-trace] ts=1025000 tid=2 event="Showed skeleton"
[wt-trace] ts=1125000 tid=1 cmd="git merge-base HEAD main" dur=500.0ms ok=true
[wt-trace] ts=1625000 tid=1 cmd="gh pr list" dur=200.0ms ok=true"#;

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
        stderr.contains("No trace entries found"),
        "Should indicate no trace entries"
    );
}

/// Test reading from a file.
#[test]
fn test_wt_perf_trace_from_file() {
    // Create a temp file with sample trace data
    let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
    let log_file = temp_dir.path().join("trace.log");

    let sample_trace = r#"[wt-trace] ts=1000000 tid=1 cmd="git rev-parse" dur=5.0ms ok=true
[wt-trace] ts=1005000 tid=1 cmd="git status" dur=10.0ms ok=true
[wt-trace] ts=1015000 tid=1 event="Skeleton displayed"
[wt-trace] ts=1015000 tid=2 cmd="git diff" dur=50.0ms ok=true"#;

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
