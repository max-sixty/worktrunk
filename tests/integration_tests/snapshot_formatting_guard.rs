//! Guard test to catch formatting violations in snapshot files
//!
//! Snapshots capture real output and are approved by reviewers. Subtle formatting
//! issues (stray blank lines, detached hints) can slip through review. This test
//! scans all snapshot files to enforce output formatting rules.
//!
//! Rules enforced:
//! - **Hints attach to their subject** — no blank line before `↳`
//! - **No double blank lines** — one blank line maximum between elements
//!
//! When this test fails:
//! 1. Fix the source code producing the bad output
//! 2. Re-run the failing test to regenerate the snapshot
//! 3. If the blank line is intentional (e.g., phase boundary before a status
//!    item that uses `↳`), add the snapshot to the allowlist with a comment

use ansi_str::AnsiStr;
use std::fs;
use std::path::Path;

/// Snapshots where a blank line before `↳` is intentional (phase boundary, not
/// a detached hint). Each entry needs a comment explaining why.
const BLANK_BEFORE_HINT_ALLOWED: &[&str] = &[
    // config show: blank line separates "Shell integration not active" diagnostic
    // from per-shell status section. The `↳` here is a status item, not a hint.
    "config_show__config_show_partial_shell_config_shows_hint",
    "config_show__config_show_unmatched_candidate_warning",
];

#[test]
fn test_no_blank_line_before_hint_in_snapshots() {
    let project_root = Path::new(env!("CARGO_MANIFEST_DIR"));

    let mut violations = Vec::new();

    for_each_snapshot(project_root, |path, content| {
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
        if BLANK_BEFORE_HINT_ALLOWED
            .iter()
            .any(|allowed| stem.contains(allowed))
        {
            return;
        }

        let clean = content.ansi_strip();
        let lines: Vec<&str> = clean.lines().collect();

        for (i, line) in lines.iter().enumerate() {
            if line.trim().starts_with('↳') && i > 0 && lines[i - 1].trim().is_empty() {
                let relative = path.strip_prefix(project_root).unwrap_or(path);
                violations.push(format!(
                    "{}:{}: blank line before hint\n  {}: {:?}\n  {}: {:?}",
                    relative.display(),
                    i + 1,
                    i,
                    lines[i - 1],
                    i + 1,
                    line,
                ));
            }
        }
    });

    if !violations.is_empty() {
        panic!(
            "Blank line before hint (↳) in {} snapshot(s):\n\n{}\n\n\
             Hints attach to their subject — no blank line between a message and its hint.\n\
             Fix the source code, not the snapshot. If the blank line is intentional,\n\
             add the snapshot to BLANK_BEFORE_HINT_ALLOWED with a comment.",
            violations.len(),
            violations.join("\n\n"),
        );
    }
}

#[test]
fn test_no_double_blank_lines_in_snapshot_output() {
    let project_root = Path::new(env!("CARGO_MANIFEST_DIR"));

    let mut violations = Vec::new();

    for_each_snapshot(project_root, |path, content| {
        // Extract output content, skipping YAML header.
        // Two formats: stdout/stderr sections (insta_cmd) or expression
        // content after closing `---`.
        for (label, text) in extract_output_sections(content) {
            let clean = text.ansi_strip();
            if clean.contains("\n\n\n") {
                let relative = path.strip_prefix(project_root).unwrap_or(path);
                violations.push(format!("{}  ({label})", relative.display()));
            }
        }
    });

    if !violations.is_empty() {
        panic!(
            "Double blank lines in {} snapshot output section(s):\n\n{}\n\n\
             One blank line maximum between output elements.",
            violations.len(),
            violations.join("\n"),
        );
    }
}

fn for_each_snapshot(project_root: &Path, mut f: impl FnMut(&Path, &str)) {
    let snap_dirs = [
        project_root.join("tests/snapshots"),
        project_root.join("tests/integration_tests/snapshots"),
    ];

    for dir in &snap_dirs {
        let Ok(entries) = fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("snap") {
                continue;
            }
            let content = fs::read_to_string(&path).unwrap();
            f(&path, &content);
        }
    }
}

/// Extract labeled output sections from a snapshot file.
///
/// Handles two formats:
/// - **insta_cmd**: sections delimited by `----- stdout -----` / `----- stderr -----`
/// - **expression**: content after the closing `---` YAML delimiter
fn extract_output_sections(content: &str) -> Vec<(&str, &str)> {
    let mut sections = Vec::new();

    // Try stdout/stderr sections (insta_cmd format)
    for section in ["stdout", "stderr"] {
        let marker = format!("----- {section} -----");
        let Some(start) = content.find(&marker) else {
            continue;
        };
        let after_marker = &content[start + marker.len()..];
        let end = after_marker
            .find("----- stdout -----")
            .or_else(|| after_marker.find("----- stderr -----"))
            .unwrap_or(after_marker.len());
        sections.push((section, &after_marker[..end]));
    }

    // If no stdout/stderr sections, try expression format (content after closing ---)
    if sections.is_empty()
        && let Some(first_delim) = content.find("---")
    {
        let after_first = &content[first_delim + 3..];
        if let Some(second_delim) = after_first.find("---") {
            let output = &after_first[second_delim + 3..];
            sections.push(("expression", output));
        }
    }

    sections
}
