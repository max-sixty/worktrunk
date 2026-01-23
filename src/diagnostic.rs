//! Diagnostic report generation for issue reporting.
//!
//! When unexpected warnings occur (timeouts, git errors, etc.), this module
//! can generate a diagnostic file that users attach to GitHub issues.
//!
//! # When Diagnostics Are Generated
//!
//! Diagnostic files are written when `-vv` is passed. Without `-vv`, the hint
//! simply tells users to run with `-vv`. This ensures the diagnostic file
//! contains useful debug information.
//!
//! # Report Format
//!
//! The report is a markdown file designed for easy pasting into GitHub issues:
//!
//! 1. **Header** — Timestamp, command that was run, and result
//! 2. **Environment** — wt version, OS, git version, shell integration
//! 3. **Worktrees** — Raw `git worktree list --porcelain` output
//! 4. **Config** — User and project config contents
//! 5. **Verbose log** — Debug log output, truncated to ~50KB if large
//!
//! # Privacy
//!
//! The report explicitly documents what IS and ISN'T included:
//!
//! **Included:** worktree paths, branch names, worktree status (prunable, locked),
//! config files, verbose logs, commit messages (in verbose logs)
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
//! use crate::diagnostic::issue_hint;
//!
//! // Show hint telling user to run with -vv
//! eprintln!("{}", hint_message(issue_hint()));
//! ```
//!
use std::borrow::Cow;
use std::path::{Path, PathBuf};

use ansi_str::AnsiStr;
use anyhow::Context;
use color_print::cformat;
use minijinja::{Environment, context};
use shell_escape::unix::escape as posix_escape;
use worktrunk::git::Repository;
use worktrunk::path::format_path_for_display;
use worktrunk::shell_exec::Cmd;
use worktrunk::styling::{eprintln, hint_message, info_message, warning_message};

use crate::cli::version_str;
use crate::output;

/// Shell-escape a path for use in POSIX shell commands.
///
/// This is used for hints like `gh gist create --web <path>` that users copy/paste.
/// We use POSIX escaping because:
/// - `gh` is a cross-platform CLI that accepts POSIX-style paths on all platforms
/// - Users on Windows typically use Git Bash or similar POSIX-compatible shells
/// - The `shell_escape` crate's platform-specific escaping produces different output
///   on Windows (cmd.exe style) vs Unix (POSIX style), causing snapshot test failures
///
/// We normalize Windows backslashes to forward slashes for consistent output.
fn shell_escape_path(path: &Path) -> Cow<'_, str> {
    let path_str = path.to_string_lossy();
    // Normalize Windows backslashes to forward slashes for consistent POSIX-style paths
    let normalized = if path_str.contains('\\') {
        Cow::Owned(path_str.replace('\\', "/"))
    } else {
        path_str
    };
    posix_escape(normalized)
}

/// Markdown template for the diagnostic report.
///
/// This template makes the report structure immediately visible.
/// Variables are filled in by `format_report()`.
const REPORT_TEMPLATE: &str = r#"## Diagnostic Report

**Generated:** {{ timestamp }}
**Command:** `{{ command }}`
**Result:** {{ context }}

<details>
<summary>Environment</summary>

```
wt {{ version }} ({{ os }} {{ arch }})
git {{ git_version }}
Shell integration: {{ shell_integration }}
```
</details>

<details>
<summary>Worktrees</summary>

```
{{ worktree_list }}
```
</details>
{% if config_show %}
<details>
<summary>Config</summary>

```
{{ config_show }}
```
</details>
{% endif %}
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
pub(crate) struct DiagnosticReport {
    /// Formatted markdown content
    content: String,
}

impl DiagnosticReport {
    /// Collect diagnostic information from the current environment.
    ///
    /// # Arguments
    /// * `repo` - Repository to collect worktree info from
    /// * `command` - The command that was run (e.g., "wt list -vv")
    /// * `context` - Context describing the result (error message or success)
    pub fn collect(repo: &Repository, command: &str, context: String) -> Self {
        let content = Self::format_report(repo, command, &context);
        Self { content }
    }

    /// Format the complete diagnostic report as markdown using minijinja template.
    fn format_report(repo: &Repository, command: &str, context: &str) -> String {
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

        // Get config show output (if available)
        let config_show = get_config_show_output(repo);

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
            command,
            context,
            version,
            os,
            arch,
            git_version,
            shell_integration,
            worktree_list,
            config_show,
            verbose_log,
        })
        .unwrap()
    }

    /// Write the diagnostic report to a file.
    fn write_file(&self, repo: &Repository) -> Option<PathBuf> {
        let log_dir = repo.wt_logs_dir();
        std::fs::create_dir_all(&log_dir).ok()?;

        let path = log_dir.join("diagnostic.md");
        std::fs::write(&path, &self.content).ok()?;

        Some(path)
    }

    /// Write the diagnostic report to a file (for -vv flag).
    ///
    /// Called from `write_if_verbose()` when verbose >= 2.
    /// Returns the path if successful, None if write failed.
    pub fn write_diagnostic_file(&self, repo: &Repository) -> Option<PathBuf> {
        self.write_file(repo)
    }
}

/// Return hint telling users to run with `-vv` for diagnostics.
///
/// This is a free function (not a method on DiagnosticReport) because it
/// doesn't require collecting diagnostic data - just returns a static hint.
///
/// TODO: Consider showing this hint automatically when any `log::warn!` occurs
/// during command execution, since runtime warnings often indicate unexpected
/// conditions that could be bugs worth reporting.
pub(crate) fn issue_hint() -> String {
    cformat!("To create a diagnostic file, run with <bright-black>-vv</>")
}

/// Write diagnostic file when -vv is used.
///
/// Called at the end of command execution. If verbose level is >= 2, writes
/// a diagnostic report to `.git/wt-logs/diagnostic.md` for issue filing.
///
/// Silently returns if:
/// - verbose < 2
/// - Not in a git repository
///
/// Warns if diagnostic file write fails.
pub(crate) fn write_if_verbose(verbose: u8, command_line: &str, error_msg: Option<&str>) {
    if verbose < 2 {
        return;
    }

    // Use Repository::current() which honors the -C flag
    let Ok(repo) = Repository::current() else {
        return;
    };

    // Check if we're actually in a git repo
    if repo.current_worktree().git_dir().is_err() {
        return;
    }

    // Build context based on success/error
    let context = match error_msg {
        Some(msg) => format!("Command failed: {msg}"),
        None => "Command completed successfully".to_string(),
    };

    // Collect and write diagnostic
    let report = DiagnosticReport::collect(&repo, command_line, context);
    match report.write_diagnostic_file(&repo) {
        Some(path) => {
            let path_display = format_path_for_display(&path);
            eprintln!(
                "{}",
                info_message(format!("Diagnostic saved: {path_display}"))
            );

            // Only show gh command if gh is installed
            if is_gh_installed() {
                let path_str = shell_escape_path(&path);
                // URL with prefilled body: ## Gist\n\n[Paste URL]\n\n## Description\n\n[Describe the issue]
                let issue_url = "https://github.com/max-sixty/worktrunk/issues/new?body=%23%23%20Gist%0A%0A%5BPaste%20gist%20URL%5D%0A%0A%23%23%20Description%0A%0A%5BDescribe%20the%20issue%5D";
                eprintln!(
                    "{}",
                    hint_message(cformat!(
                        "To report a bug, create a secret gist with <bright-black>gh gist create --web {path_str}</> and reference it from an issue at <bright-black>{issue_url}</>"
                    ))
                );
            }
        }
        None => {
            eprintln!("{}", warning_message("Failed to write diagnostic file"));
        }
    }
}

/// Check if the GitHub CLI (gh) is installed.
fn is_gh_installed() -> bool {
    Cmd::new("gh")
        .arg("--version")
        .run()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Strip ANSI escape codes from a string.
///
/// Used to clean terminal-formatted text for markdown output.
fn strip_ansi_codes(s: &str) -> String {
    s.ansi_strip().into_owned()
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
    let output = Cmd::new("git")
        .arg("--version")
        .run()
        .context("Failed to run git --version")?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let version = stdout
        .trim()
        .strip_prefix("git version ")
        .unwrap_or(stdout.trim())
        .to_string();

    Ok(version)
}

/// Get config show output for diagnostic.
///
/// Returns a summary of user and project config files.
fn get_config_show_output(repo: &Repository) -> Option<String> {
    let mut output = String::new();

    // User config
    if let Some(user_config_path) = worktrunk::config::get_config_path() {
        output.push_str(&format_config_section(&user_config_path, "User config"));
    }

    // Project config
    if let Ok(root) = repo.current_worktree().root() {
        let project_config_path = root.join(".config/wt.toml");
        output.push_str(&format!(
            "\n{}",
            format_config_section(&project_config_path, "Project config")
        ));
    }

    if output.is_empty() {
        None
    } else {
        Some(output.trim().to_string())
    }
}

/// Format a config file section for diagnostic output.
fn format_config_section(path: &std::path::Path, label: &str) -> String {
    let mut output = format!("{}: {}\n", label, path.display());
    if path.exists() {
        match std::fs::read_to_string(path) {
            Ok(content) if content.trim().is_empty() => output.push_str("(empty file)\n"),
            Ok(content) => {
                // Include content, but truncate if very long
                let content = if content.len() > 4000 {
                    format!("{}...\n(truncated)", &content[..4000])
                } else {
                    content
                };
                output.push_str(&content);
                if !output.ends_with('\n') {
                    output.push('\n');
                }
            }
            Err(e) => output.push_str(&format!("(read failed: {})\n", e)),
        }
    } else {
        output.push_str("(file not found)\n");
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_format_config_section_file_not_found() {
        let result = format_config_section(std::path::Path::new("/nonexistent/path.toml"), "Test");
        assert!(result.contains("Test: /nonexistent/path.toml"));
        assert!(result.contains("(file not found)"));
    }

    #[test]
    fn test_format_config_section_empty_file() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("empty.toml");
        std::fs::write(&path, "").unwrap();

        let result = format_config_section(&path, "Test");
        assert!(result.contains("(empty file)"));
    }

    #[test]
    fn test_format_config_section_with_content() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("config.toml");
        std::fs::write(&path, "key = \"value\"\n").unwrap();

        let result = format_config_section(&path, "Test");
        assert!(result.contains("key = \"value\""));
    }

    #[test]
    fn test_format_config_section_adds_trailing_newline() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("config.toml");
        std::fs::write(&path, "no-newline").unwrap();

        let result = format_config_section(&path, "Test");
        assert!(result.ends_with('\n'));
    }

    #[test]
    fn test_format_config_section_truncates_long_content() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("big.toml");
        let content = "x".repeat(5000);
        std::fs::write(&path, &content).unwrap();

        let result = format_config_section(&path, "Test");
        assert!(result.contains("(truncated)"));
        assert!(result.len() < 5000);
    }

    #[test]
    fn test_strip_ansi_codes() {
        // Build ANSI codes programmatically to avoid lint
        let esc = '\x1b';
        let input = format!("{esc}[31mred{esc}[0m and {esc}[32mgreen{esc}[0m");
        let result = strip_ansi_codes(&input);
        assert_eq!(result, "red and green");
    }

    #[test]
    fn test_truncate_log_small_content() {
        let content = "small log content";
        let result = truncate_log(content);
        assert_eq!(result, content);
    }

    #[test]
    fn test_truncate_log_large_content() {
        let content = "x".repeat(60 * 1024); // 60KB
        let result = truncate_log(&content);
        assert!(result.starts_with("(log truncated to last ~50KB)"));
        assert!(result.len() < 55 * 1024);
    }

    #[test]
    fn test_shell_escape_path_unix_style() {
        // Unix paths with no special chars pass through unchanged
        let path = Path::new("/home/user/project/.git/wt-logs/diagnostic.md");
        let result = shell_escape_path(path);
        assert_eq!(result, "/home/user/project/.git/wt-logs/diagnostic.md");
    }

    #[test]
    fn test_shell_escape_path_windows_backslashes() {
        // Windows backslashes are normalized to forward slashes
        let path = Path::new(r"D:\a\worktrunk\.git\wt-logs\diagnostic.md");
        let result = shell_escape_path(path);
        // The colon in D: requires quoting in POSIX shells
        assert!(result.contains("D:/a/worktrunk"));
        assert!(!result.contains('\\'));
    }

    #[test]
    fn test_shell_escape_path_special_chars() {
        // Paths with spaces or special chars get quoted
        let path = Path::new("/path/with spaces/file.md");
        let result = shell_escape_path(path);
        assert!(result.starts_with('\'') || result.contains("\\ "));
    }
}
