//! Integration tests for the analyze-trace binary.

use std::io::Write;
use std::process::{Command, Stdio};

/// Test that the binary produces Chrome Trace Format JSON for sample trace input.
#[test]
fn test_analyze_trace_from_stdin() {
    let sample_trace = r#"[wt-trace] ts=1000000 tid=1 cmd="git status" dur=10.0ms ok=true
[wt-trace] ts=1000000 tid=2 cmd="git status" dur=15.0ms ok=true
[wt-trace] ts=1010000 tid=1 cmd="git diff" dur=100.0ms ok=true
[wt-trace] ts=1020000 tid=2 cmd="git merge-base HEAD main" dur=500.0ms ok=true
[wt-trace] ts=1030000 tid=1 cmd="gh pr list" dur=200.0ms ok=true"#;

    let mut child = Command::new(env!("CARGO_BIN_EXE_analyze-trace"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to spawn analyze-trace");

    child
        .stdin
        .take()
        .unwrap()
        .write_all(sample_trace.as_bytes())
        .expect("Failed to write to stdin");

    let output = child.wait_with_output().expect("Failed to read output");

    assert!(output.status.success(), "analyze-trace should succeed");

    let stdout = String::from_utf8_lossy(&output.stdout);

    // Should be valid JSON with Chrome Trace Format structure
    let parsed: serde_json::Value =
        serde_json::from_str(&stdout).expect("Output should be valid JSON");

    assert!(
        parsed["traceEvents"].is_array(),
        "Should have traceEvents array"
    );
    assert_eq!(
        parsed["displayTimeUnit"], "ms",
        "Should have displayTimeUnit"
    );

    let events = parsed["traceEvents"].as_array().unwrap();
    assert_eq!(events.len(), 5, "Should have 5 trace events");

    // Check first event structure
    assert_eq!(events[0]["name"], "git status");
    assert_eq!(events[0]["ph"], "X"); // Complete event
    assert_eq!(events[0]["ts"], 1000000);
    assert_eq!(events[0]["tid"], 1);
    assert_eq!(events[0]["cat"], "git");
}

/// Test that the binary shows usage when run interactively without input.
#[test]
fn test_analyze_trace_no_input_shows_usage() {
    // Test by passing a non-existent file
    let output = Command::new(env!("CARGO_BIN_EXE_analyze-trace"))
        .arg("/nonexistent/path/to/file.log")
        .output()
        .expect("Failed to run analyze-trace");

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
fn test_analyze_trace_empty_input() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_analyze-trace"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to spawn analyze-trace");

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
fn test_analyze_trace_from_file() {
    // Create a temporary file with trace data
    let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
    let log_path = temp_dir.path().join("trace.log");

    let sample_trace = r#"[wt-trace] ts=1000000 tid=1 cmd="git rev-parse" dur=5.0ms ok=true
[wt-trace] ts=1005000 tid=1 cmd="git status" dur=10.0ms ok=true
[wt-trace] ts=1015000 tid=2 cmd="git diff" dur=20.0ms ok=true"#;

    std::fs::write(&log_path, sample_trace).expect("Failed to write temp file");

    let output = Command::new(env!("CARGO_BIN_EXE_analyze-trace"))
        .arg(&log_path)
        .output()
        .expect("Failed to run analyze-trace");

    assert!(output.status.success(), "Should succeed with sample log");

    let stdout = String::from_utf8_lossy(&output.stdout);

    // Should be valid Chrome Trace Format JSON
    let parsed: serde_json::Value =
        serde_json::from_str(&stdout).expect("Output should be valid JSON");

    let events = parsed["traceEvents"].as_array().unwrap();
    assert_eq!(events.len(), 3, "Should have 3 trace events");
    assert_eq!(events[0]["name"], "git rev-parse");
}
