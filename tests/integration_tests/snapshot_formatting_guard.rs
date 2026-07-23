//! Guard tests over the committed snapshot corpus.
//!
//! Snapshots capture real output and are approved by reviewers. Subtle
//! problems slip through review: formatting issues (stray blank lines,
//! detached hints), and host-specific paths that insta never compares.
//! These tests scan every committed `.snap` file to enforce the rules.
//!
//! Rules enforced:
//! - **Hints attach to their subject** — no blank line before `↳`
//! - **No double blank lines** — one blank line maximum between elements
//! - **No host-specific paths** — insta compares only snapshot *content*
//!   (stdout/stderr/exit code); the `info:` block insta-cmd records
//!   (`args:`, `env:`) is metadata that is written but never compared, and
//!   `add_filter` doesn't apply to it. A test missing a redaction therefore
//!   bakes the generating machine's paths into the committed file while
//!   passing everywhere — surfacing only as churn when someone regenerates
//!   the snapshot on another machine (and under libtest, the `repo`
//!   fixture's leaked settings binding can mask the omission; see `repo()`
//!   in `tests/common`). This rule makes the leak fail deterministically.
//!
//! When a formatting rule fails:
//! 1. Fix the source code producing the bad output
//! 2. Re-run the failing test to regenerate the snapshot
//! 3. If the blank line is intentional (e.g., phase boundary before a status
//!    item that uses `↳`), add the snapshot to the allowlist with a comment
//!
//! When the host-path rule fails: add a redaction — env values go in
//! `add_standard_env_redactions`, path arguments are covered by the
//! `.args[]` redaction in `add_repo_and_worktree_path_filters` (both in
//! `tests/common/mod.rs`); see tests/CLAUDE.md "Snapshot env drift".

use ansi_str::AnsiStr;
use regex::Regex;
use std::fs;
use std::path::Path;
use std::sync::LazyLock;

/// Snapshots where a blank line before `↳` is intentional (phase boundary, not
/// a detached hint). Each entry needs a comment explaining why.
const BLANK_BEFORE_HINT_ALLOWED: &[&str] = &[
    // config show: blank line separates "Shell integration not active" diagnostic
    // from per-shell status section. The `↳` here is a status item, not a hint.
    "config_show__config_show_partial_shell_config_shows_hint",
    "config_show__config_show_unmatched_candidate_warning",
    "config_show__config_show_unmatched_candidate_not_suppressed_by_wrapper",
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

/// Markers that only ever come from the generating machine: macOS per-user
/// temp dirs, the LLVM profile fallback dir (always under the host temp
/// dir), and tempfile's random `.tmpXXXXXX` directories.
static HOST_MARKERS: LazyLock<[Regex; 3]> = LazyLock::new(|| {
    [
        Regex::new(r"/var/folders/").unwrap(),
        Regex::new(r"wt-test-profraw").unwrap(),
        Regex::new(r"\.tmp[A-Za-z0-9]{6}").unwrap(),
    ]
});

/// Home-style paths are allowed only for the deliberate fake users in docs
/// examples and mocked output; a real username means an unredacted $HOME
/// leaked. `{1,2}` separators: YAML double-quoted scalars escape
/// backslashes, so a Windows path arrives as `C:\\Users\\name`.
static HOME_PATH: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[/\\]{1,2}(?:Users|home)[/\\]{1,2}([A-Za-z0-9._-]+)").unwrap());

const FAKE_USERS: &[&str] = &["me", "user"];

fn line_has_host_specific_path(line: &str) -> bool {
    HOST_MARKERS.iter().any(|re| re.is_match(line))
        || HOME_PATH
            .captures_iter(line)
            .any(|c| !FAKE_USERS.contains(&&c[1]))
}

#[test]
fn test_no_host_specific_paths_in_snapshots() {
    let project_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut violations = Vec::new();

    for_each_snapshot(project_root, |path, content| {
        for (i, line) in content.lines().enumerate() {
            if line_has_host_specific_path(line) {
                let relative = path.strip_prefix(project_root).unwrap_or(path);
                violations.push(format!("{}:{}: {}", relative.display(), i + 1, line.trim()));
            }
        }
    });

    if !violations.is_empty() {
        panic!(
            "Host-specific paths in {} committed snapshot line(s):\n\n{}\n\n\
             These churn whenever the snapshot is regenerated on another machine.\n\
             Add a redaction: env values in `add_standard_env_redactions`, path\n\
             arguments via the `.args[]` redaction in `add_repo_and_worktree_path_filters`\n\
             (tests/common/mod.rs); see tests/CLAUDE.md \"Snapshot env drift\".\n\
             Deliberate example paths use the fake users in FAKE_USERS.",
            violations.len(),
            violations.join("\n"),
        );
    }
}

/// Visit every committed `.snap` file. Snapshot dirs live under `src`
/// (unit tests), `tests` (integration tests), and `docs` (demo fixtures).
fn for_each_snapshot(project_root: &Path, mut f: impl FnMut(&Path, &str)) {
    let mut seen = 0usize;
    for root in ["src", "tests", "docs"] {
        visit_snap_files(&project_root.join(root), &mut |path, content| {
            seen += 1;
            f(path, content);
        });
    }
    // Every caller asserts absence over the corpus (~1200 files); an empty
    // walk — say, after a directory-layout change — would pass vacuously.
    assert!(
        seen > 500,
        "expected the full snapshot corpus, saw {seen} files"
    );
}

fn visit_snap_files(dir: &Path, f: &mut impl FnMut(&Path, &str)) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            visit_snap_files(&path, f);
        } else if path.extension().and_then(|s| s.to_str()) == Some("snap") {
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
    // Match `---` only at line boundaries to avoid false matches on content like
    // `--- a/file.txt` in git diffs.
    if sections.is_empty() {
        let first_delim = if content.starts_with("---\n") {
            Some(0)
        } else {
            content.find("\n---\n").map(|pos| pos + 1)
        };
        if let Some(pos) = first_delim {
            let after_first = &content[pos + 3..];
            if let Some(nl_pos) = after_first.find("\n---\n") {
                let output = &after_first[nl_pos + 4..];
                sections.push(("expression", output));
            }
        }
    }

    sections
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The corpus test passes vacuously if the detector rots — pin its
    /// sensitivity on both sides. Leak lines are the shapes actually found
    /// in committed snapshots (#3009 and the `args:` leak this fixed).
    #[test]
    fn test_host_specific_path_detection() {
        let leaks = [
            // macOS per-user temp dir (env value, the #3009 class)
            "LLVM_PROFILE_FILE: /var/folders/v3/8k5q0_6j3/T/wt-test-profraw/cov.profraw",
            // tempfile-crate dir in the args: block
            "- /private/var/folders/wf/s6ycxvvs/T/.tmpSeGxzx/repo",
            // tempfile-crate dir under /tmp (Linux)
            "path: /tmp/.tmpAbC123/repo",
            // real home dirs, Unix and YAML-escaped Windows
            "HOME: /Users/maximilian/workspace",
            "HOME: /home/runner/work",
            r#"USERPROFILE: "C:\\Users\\runneradmin\\AppData""#,
        ];
        for line in leaks {
            assert!(line_has_host_specific_path(line), "should flag: {line}");
        }

        let deterministic = [
            // deliberate fake users in docs examples and mocked output
            "e.g., /Users/me/code/myproject",
            "To /Users/user/workspace/repo/.git",
            r#"PSModulePath: "C:\\Users\\user\\Documents""#,
            // machine-independent literals and placeholders
            "WORKTRUNK_CONFIG_PATH: /nonexistent/wt/config.toml",
            "LLVM_PROFILE_FILE: \"[LLVM_PROFILE_FILE]\"",
            "WORKTRUNK_SYSTEM_CONFIG_PATH: /etc/xdg/worktrunk/config.toml",
        ];
        for line in deterministic {
            assert!(!line_has_host_specific_path(line), "should allow: {line}");
        }
    }

    #[test]
    fn test_extract_output_sections_ignores_mid_line_dashes() {
        // Simulate a snapshot whose output body contains `--- a/file.txt` (git diff).
        // The old code matched `---` as a bare substring and would split incorrectly.
        let content = "\
---
source: tests/some_test.rs
expression: output
---
diff --git a/file.txt b/file.txt
--- a/file.txt
+++ b/file.txt
@@ -1 +1 @@
-old
+new
";
        let sections = extract_output_sections(content);
        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].0, "expression");
        assert!(
            sections[0].1.contains("--- a/file.txt"),
            "mid-line `---` should be part of the output, not a delimiter"
        );
    }

    #[test]
    fn test_extract_output_sections_expression_format() {
        let content = "\
---
source: tests/some_test.rs
expression: output
---
hello world
";
        let sections = extract_output_sections(content);
        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].0, "expression");
        assert_eq!(sections[0].1.trim(), "hello world");
    }
}
