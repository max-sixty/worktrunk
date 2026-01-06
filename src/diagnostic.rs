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
use std::path::PathBuf;
use std::process::{Command, Stdio};

use anyhow::Context;
use color_print::cformat;
use minijinja::{Environment, context};
use worktrunk::git::Repository;
use worktrunk::path::format_path_for_display;
use worktrunk::shell_exec::run;

use crate::cli::version_str;
use crate::output;

/// Markdown template for the diagnostic report.
///
/// This template makes the report structure immediately visible.
/// Variables are filled in by `format_report()`.
const REPORT_TEMPLATE: &str = r#"## Diagnostic Report

**Generated:** {{ timestamp }}
**Context:** {{ context }}

### What's included

- wt version, OS, git version
- Worktree paths and branch names
- Worktree status (prunable, locked, etc.)
- Shell integration status
- Verbose logs (if run with --verbose)
- Commit messages (in verbose logs)

Does NOT contain: file contents, credentials.

<details>
<summary>Diagnostic data</summary>

```
wt {{ version }} ({{ os }} {{ arch }})
git {{ git_version }}
Shell integration: {{ shell_integration }}

--- git worktree list --porcelain ---
{{ worktree_list }}
```
</details>
{% if verbose_log %}
<details>
<summary>Verbose log</summary>

```
{{ verbose_log }}
```
</details>
{% endif %}
"#;

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

    /// Format the complete diagnostic report as markdown using minijinja template.
    fn format_report(repo: &Repository, context: &str) -> String {
        // Strip ANSI codes from context - the diagnostic is a markdown file for GitHub
        let context = strip_ansi_codes(context);

        // Collect data for template
        let timestamp = worktrunk::utils::now_iso8601();
        let version = version_str();
        let os = std::env::consts::OS;
        let arch = std::env::consts::ARCH;
        let git_version = get_git_version().unwrap_or_else(|_| "(unknown)".to_string());
        let shell_integration = if output::is_shell_integration_active() {
            "active"
        } else {
            "inactive"
        };
        let worktree_list = repo
            .run_command(&["worktree", "list", "--porcelain"])
            .map(|s| s.trim_end().to_string())
            .unwrap_or_else(|_| "(failed to get worktree list)".to_string());

        // Get verbose log content (if available)
        let verbose_log = crate::verbose_log::log_file_path()
            .and_then(|path| std::fs::read_to_string(&path).ok())
            .map(|content| truncate_log(content.trim()))
            .filter(|s| !s.is_empty());

        // Render template
        let env = Environment::new();
        let tmpl = env.template_from_str(REPORT_TEMPLATE).unwrap();
        tmpl.render(context! {
            timestamp,
            context,
            version,
            os,
            arch,
            git_version,
            shell_integration,
            worktree_list,
            verbose_log,
        })
        .unwrap()
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

/// Truncate verbose log to ~50KB if it's too large.
///
/// Keeps the last ~50KB of the log, cutting at a line boundary.
fn truncate_log(content: &str) -> String {
    const MAX_LOG_SIZE: usize = 50 * 1024;
    if content.len() <= MAX_LOG_SIZE {
        return content.to_string();
    }

    let start = content.len() - MAX_LOG_SIZE;
    // Find the next newline to avoid cutting mid-line
    let start = content[start..]
        .find('\n')
        .map(|i| start + i + 1)
        .unwrap_or(start);

    format!("(log truncated to last ~50KB)\n{}", &content[start..])
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
