//! Diagnostic report generation for issue reporting.
//!
//! When unexpected warnings occur (timeouts, git errors, etc.), this module
//! can generate a diagnostic file that users attach to GitHub issues.
//!
//! # When Diagnostics Are Generated
//!
//! Diagnostic files are only written when `--verbose` is passed. Without verbose
//! logging, the hint simply tells users to re-run with `--verbose`. This ensures
//! the diagnostic file contains useful debug information.
//!
//! # Report Format
//!
//! The report is a markdown file designed for easy pasting into GitHub issues:
//!
//! 1. **Header** — Timestamp and context describing the issue
//! 2. **Diagnostic data** — Collapsed `<details>` block with:
//!    - wt version, OS, architecture
//!    - git version
//!    - Shell integration status
//!    - Raw `git worktree list --porcelain` output
//! 3. **Verbose log** — Debug log output, truncated to ~50KB if large
//!
//! # Privacy
//!
//! The report explicitly documents what IS and ISN'T included:
//!
//! **Included:** worktree paths, branch names, worktree status (prunable, locked),
//! verbose logs, commit messages (in verbose logs)
//!
//! **Not included:** file contents, credentials
//!
//! # File Location
//!
//! Reports are written to `.git/wt-logs/diagnostic.md` in the main worktree.
//! Verbose logs go to `.git/wt-logs/verbose.log`.
//!
//! # Usage
//!
//! ```rust,ignore
//! use crate::diagnostic::DiagnosticReport;
//!
//! // Show hint (writes diagnostic file only if --verbose was used)
//! let report = DiagnosticReport::collect(&repo, "Some git operations failed".into());
//! output::print(hint_message(report.issue_hint(&repo)))?;
//! ```
//!
use std::fmt::Write as _;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use anyhow::Context;
use color_print::cformat;
use worktrunk::git::Repository;
use worktrunk::path::format_path_for_display;
use worktrunk::shell_exec::run;

use crate::cli::version_str;
use crate::output;

/// Collected diagnostic information for issue reporting.
pub struct DiagnosticReport {
    /// Formatted markdown content
    content: String,
}

impl DiagnosticReport {
    /// Collect diagnostic information from the current environment.
    ///
    /// # Arguments
    /// * `repo` - Repository to collect worktree info from
    /// * `context` - Context describing the issue (error message, affected item)
    pub fn collect(repo: &Repository, context: String) -> Self {
        let content = Self::format_report(repo, &context);
        Self { content }
    }

    /// Format the complete diagnostic report as markdown.
    fn format_report(repo: &Repository, context: &str) -> String {
        let mut out = String::new();

        // Strip ANSI codes from context - the diagnostic is a markdown file for GitHub
        let context = strip_ansi_codes(context);

        // Header
        writeln!(out, "## Diagnostic Report\n").unwrap();
        writeln!(out, "**Generated:** {}  ", chrono_now()).unwrap();
        writeln!(out, "**Context:** {}  ", context).unwrap();
        writeln!(out).unwrap();

        // Privacy notice
        writeln!(out, "### What's included\n").unwrap();
        writeln!(out, "- wt version, OS, git version").unwrap();
        writeln!(out, "- Worktree paths and branch names").unwrap();
        writeln!(out, "- Worktree status (prunable, locked, etc.)").unwrap();
        writeln!(out, "- Shell integration status").unwrap();
        writeln!(out, "- Verbose logs (if run with --verbose)").unwrap();
        writeln!(out, "- Commit messages (in verbose logs)").unwrap();
        writeln!(out).unwrap();
        writeln!(out, "Does NOT contain: file contents, credentials.\n").unwrap();

        // Diagnostic data in details block
        writeln!(out, "<details>").unwrap();
        writeln!(out, "<summary>Diagnostic data</summary>\n").unwrap();
        writeln!(out, "```").unwrap();

        // Version info
        let version = version_str();
        let os = std::env::consts::OS;
        let arch = std::env::consts::ARCH;
        writeln!(out, "wt {version} ({os} {arch})").unwrap();

        // Git version
        if let Ok(git_version) = get_git_version() {
            writeln!(out, "git {git_version}").unwrap();
        }

        // Shell integration
        let shell_active = output::is_shell_integration_active();
        writeln!(
            out,
            "Shell integration: {}",
            if shell_active { "active" } else { "inactive" }
        )
        .unwrap();

        writeln!(out).unwrap();

        // Raw git worktree list output (most useful for debugging)
        writeln!(out, "--- git worktree list --porcelain ---").unwrap();
        if let Ok(worktree_output) = repo.run_command(&["worktree", "list", "--porcelain"]) {
            // trim_end ensures no trailing whitespace, then we add exactly one newline
            writeln!(out, "{}", worktree_output.trim_end()).unwrap();
        } else {
            writeln!(out, "(failed to get worktree list)").unwrap();
        }

        writeln!(out, "```").unwrap();
        writeln!(out, "</details>").unwrap();

        // Verbose logs (if available)
        if let Some(log_path) = crate::verbose_log::log_file_path()
            && let Ok(log_content) = std::fs::read_to_string(&log_path)
        {
            let log_content = log_content.trim();
            if !log_content.is_empty() {
                writeln!(out).unwrap();
                writeln!(out, "<details>").unwrap();
                writeln!(out, "<summary>Verbose log</summary>\n").unwrap();
                writeln!(out, "```").unwrap();
                // Limit log size to ~50KB to avoid huge reports
                const MAX_LOG_SIZE: usize = 50 * 1024;
                if log_content.len() > MAX_LOG_SIZE {
                    writeln!(out, "(log truncated to last ~50KB)").unwrap();
                    let start = log_content.len() - MAX_LOG_SIZE;
                    // Find the next newline to avoid cutting mid-line
                    let start = log_content[start..]
                        .find('\n')
                        .map(|i| start + i + 1)
                        .unwrap_or(start);
                    writeln!(out, "{}", &log_content[start..]).unwrap();
                } else {
                    writeln!(out, "{}", log_content).unwrap();
                }
                writeln!(out, "```").unwrap();
                writeln!(out, "</details>").unwrap();
            }
        }

        out
    }

    /// Write diagnostic file (if verbose) and return issue reporting hint.
    ///
    /// If verbose logging is active: writes diagnostic file, returns hint with gh command.
    /// Otherwise: returns hint to re-run with --verbose (no file written).
    pub fn issue_hint(&self, repo: &Repository) -> String {
        if !crate::verbose_log::is_active() {
            return cformat!("To create a diagnostic file, re-run with <bright-black>--verbose</>");
        }

        // Write the diagnostic file
        let Some(path) = self.write_file(repo) else {
            return "Failed to write diagnostic file".to_string();
        };

        let path_display = format_path_for_display(&path);
        let mut hint = format!("Diagnostic saved: {path_display}");

        if is_gh_installed() {
            // Escape single quotes for shell: 'it'\''s' -> it's
            let path_str = path.to_string_lossy().replace('\'', "'\\''");
            hint.push_str(&cformat!(
                "\n   <bright-black>gh issue create -R max-sixty/worktrunk -t 'Bug report' --body-file '{path_str}'</>"
            ));
        }

        hint
    }

    /// Write the diagnostic report to a file.
    fn write_file(&self, repo: &Repository) -> Option<PathBuf> {
        let log_dir = repo.wt_logs_dir().ok()?;
        std::fs::create_dir_all(&log_dir).ok()?;

        let path = log_dir.join("diagnostic.md");
        std::fs::write(&path, &self.content).ok()?;

        Some(path)
    }
}

/// Check if the GitHub CLI (gh) is installed.
fn is_gh_installed() -> bool {
    let mut cmd = Command::new("gh");
    cmd.args(["--version"]);
    cmd.stdin(Stdio::null());

    run(&mut cmd, None)
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Strip ANSI escape codes from a string.
///
/// Used to clean terminal-formatted text for markdown output.
fn strip_ansi_codes(s: &str) -> String {
    // Match SGR (Select Graphic Rendition) sequences: ESC [ <params> m
    // This covers colors, bold, dim, etc.
    let re = regex::Regex::new(r"\x1b\[[0-9;]*m").unwrap();
    re.replace_all(s, "").into_owned()
}

/// Get the current time as ISO 8601 string.
///
/// Respects `SOURCE_DATE_EPOCH` for reproducible test snapshots.
fn chrono_now() -> String {
    let timestamp = worktrunk::utils::get_now() as i64;
    chrono::DateTime::from_timestamp(timestamp, 0)
        .unwrap_or_else(chrono::Utc::now)
        .format("%Y-%m-%dT%H:%M:%SZ")
        .to_string()
}

/// Get git version string.
fn get_git_version() -> anyhow::Result<String> {
    let mut cmd = Command::new("git");
    cmd.args(["--version"]);
    cmd.stdin(Stdio::null());

    let output = run(&mut cmd, None).context("Failed to run git --version")?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let version = stdout
        .trim()
        .strip_prefix("git version ")
        .unwrap_or(stdout.trim())
        .to_string();

    Ok(version)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_gh_installed_returns_bool() {
        // Just verify it doesn't panic and returns a bool
        let _ = is_gh_installed();
    }
}
