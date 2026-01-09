//! Integration tests for the analyze-trace binary.

use std::io::Write;
use std::process::{Command, Stdio};

/// Test that the binary produces expected output for sample trace input.
#[test]
fn test_analyze_trace_from_stdin() {
    let sample_trace = r#"[wt-trace] cmd="git status" dur=10.0ms ok=true
[wt-trace] cmd="git status" dur=15.0ms ok=true
[wt-trace] cmd="git diff" dur=100.0ms ok=true
[wt-trace] cmd="git merge-base HEAD main" dur=500.0ms ok=true
[wt-trace] cmd="gh pr list" dur=200.0ms ok=true"#;

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
    // Check key elements of the output
    assert!(
        stdout.contains("TRACE PERFORMANCE ANALYSIS"),
        "Should have header"
    );
    assert!(
        stdout.contains("git merge-base"),
        "Should show git merge-base"
    );
    assert!(stdout.contains("git status"), "Should show git status");
    assert!(stdout.contains("TOTAL"), "Should show total row");
}

/// Test that the binary shows usage when run interactively without input.
#[test]
fn test_analyze_trace_no_input_shows_usage() {
    // Use --help to test non-interactive path without hanging
    // The binary doesn't have --help, so we test by passing a non-existent file
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
    // Use the sample log file from the testdata directory
    let sample_log_path = concat!(env!("CARGO_MANIFEST_DIR"), "/src/trace/testdata/sample.log");

    let output = Command::new(env!("CARGO_BIN_EXE_analyze-trace"))
        .arg(sample_log_path)
        .output()
        .expect("Failed to run analyze-trace");

    assert!(output.status.success(), "Should succeed with sample log");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("TRACE PERFORMANCE ANALYSIS"),
        "Should have header"
    );
    assert!(
        stdout.contains("git rev-parse"),
        "Should show git rev-parse (most common in sample)"
    );
    assert!(stdout.contains("TOTAL"), "Should show total row");
}
