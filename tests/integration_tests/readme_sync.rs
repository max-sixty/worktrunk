//! README synchronization test
//!
//! Verifies that README.md examples stay in sync with their source snapshots.
//! This replaces the Python `dev/update-readme.py` script with a native Rust test.
//!
//! Run with: `cargo test --test integration readme_sync`
//!
//! To update README when snapshots change:
//! 1. Run this test to see which sections are out of sync
//! 2. Copy the expected content from the test output into README.md

use regex::Regex;
use std::fs;
use std::path::Path;
use std::sync::LazyLock;

/// Regex to find README snapshot markers
static MARKER_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?s)<!-- README:snapshot:([^\s]+) -->\n```\w*\n(?:\$ [^\n]+\n)?(.*?)```\n<!-- README:end -->",
    )
    .expect("Invalid marker regex")
});

/// Regex to strip ANSI escape codes (actual escape sequences)
static ANSI_ESCAPE_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\x1b\[[0-9;]*m").expect("Invalid ANSI regex"));

/// Regex to strip literal bracket notation (as stored in snapshots)
static ANSI_LITERAL_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[[0-9;]*m").expect("Invalid literal ANSI regex"));

/// Regex for SHA placeholder
static SHA_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[SHA\]").expect("Invalid SHA regex"));

/// Regex for HASH placeholder (used by shell_wrapper tests)
static HASH_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[HASH\]").expect("Invalid HASH regex"));

/// Regex for TMPDIR paths
static TMPDIR_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[TMPDIR\]/test-repo\.([^\s/]+)").expect("Invalid TMPDIR regex"));

/// Regex for REPO placeholder
static REPO_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[REPO\]").expect("Invalid REPO regex"));

/// Strip ANSI escape codes from text
fn strip_ansi(text: &str) -> String {
    let text = ANSI_ESCAPE_REGEX.replace_all(text, "");
    ANSI_LITERAL_REGEX.replace_all(&text, "").to_string()
}

/// Parse content from an insta snapshot file
fn parse_snapshot(path: &Path) -> Result<String, String> {
    let content = fs::read_to_string(path)
        .map_err(|e| format!("Failed to read {}: {}", path.display(), e))?;

    // Remove YAML front matter
    let content = if content.starts_with("---") {
        let parts: Vec<&str> = content.splitn(3, "---").collect();
        if parts.len() >= 3 {
            parts[2].trim().to_string()
        } else {
            content
        }
    } else {
        content
    };

    // Handle insta_cmd format with stdout/stderr sections
    let content = if content.contains("----- stdout -----") {
        // Extract stdout section
        let stdout = if let Some(start) = content.find("----- stdout -----\n") {
            let after_header = &content[start + "----- stdout -----\n".len()..];
            if let Some(end) = after_header.find("----- stderr -----") {
                after_header[..end].trim_end().to_string()
            } else {
                after_header.trim_end().to_string()
            }
        } else {
            String::new()
        };

        // Extract stderr section
        let stderr = if let Some(start) = content.find("----- stderr -----\n") {
            let after_header = &content[start + "----- stderr -----\n".len()..];
            if let Some(end) = after_header.find("----- ") {
                after_header[..end].trim_end().to_string()
            } else {
                after_header.trim_end().to_string()
            }
        } else {
            String::new()
        };

        // Use stdout if it has content, otherwise stderr
        if !stdout.is_empty() { stdout } else { stderr }
    } else {
        content
    };

    // Strip ANSI codes
    Ok(strip_ansi(&content))
}

/// Normalize snapshot output for README display
fn normalize_for_readme(content: &str) -> String {
    let content = SHA_REGEX.replace_all(content, "a1b2c3d");
    let content = HASH_REGEX.replace_all(&content, "a1b2c3d");
    let content = TMPDIR_REGEX.replace_all(&content, "../repo.$1");
    let content = REPO_REGEX.replace_all(&content, "../repo");

    // Trim trailing whitespace from each line (matches pre-commit behavior)
    content
        .lines()
        .map(|line| line.trim_end())
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn test_readme_examples_are_in_sync() {
    let project_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let readme_path = project_root.join("README.md");

    let readme_content = fs::read_to_string(&readme_path).expect("Failed to read README.md");

    let mut errors = Vec::new();
    let mut checked = 0;

    for cap in MARKER_PATTERN.captures_iter(&readme_content) {
        let snap_path = cap.get(1).unwrap().as_str();
        // Normalize README content: trim trailing whitespace from each line
        let current_content = cap
            .get(2)
            .unwrap()
            .as_str()
            .lines()
            .map(|line| line.trim_end())
            .collect::<Vec<_>>()
            .join("\n");
        let current_content = current_content.trim();
        checked += 1;

        let full_path = project_root.join(snap_path);

        // Parse and normalize the snapshot
        let expected = match parse_snapshot(&full_path) {
            Ok(content) => normalize_for_readme(&content),
            Err(e) => {
                errors.push(format!("❌ {}: {}", snap_path, e));
                continue;
            }
        };

        let expected = expected.trim();

        // Compare
        if current_content != expected {
            errors.push(format!(
                "❌ {} is out of sync\n\n--- Current (in README) ---\n{}\n\n--- Expected (from snapshot) ---\n{}\n",
                snap_path, current_content, expected
            ));
        }
    }

    if checked == 0 {
        panic!("No README:snapshot markers found in README.md");
    }

    if !errors.is_empty() {
        panic!(
            "README examples are out of sync with snapshots:\n\n{}\n\n\
            To fix: Update the README sections with the expected content above.\n\
            Checked {} markers, {} out of sync.",
            errors.join("\n"),
            checked,
            errors.len()
        );
    }

    // Test passes implicitly - no errors found
}
