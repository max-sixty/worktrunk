//! Directive output mode for shell integration
//!
//! # How Shell Integration Works
//!
//! Worktrunk uses a simple shell script protocol for shell integration. When running
//! with `--internal` flag (invoked by shell wrapper), all user-facing messages stream
//! to stderr in real-time, and a shell script is emitted to stdout at the very end.
//!
//! ## Protocol
//!
//! Running `wt switch --internal my-branch` outputs:
//!
//! **stderr** (streams directly to terminal in real-time):
//! ```text
//! ✅ Created new worktree for my-branch at ~/worktrees/my-branch
//! ```
//!
//! **stdout** (shell script emitted at the end):
//! ```text
//! cd '/path/to/worktree'
//! ```
//!
//! The shell wrapper captures stdout via command substitution and evals it:
//! ```bash
//! wt() {
//!     local script exit_code=0
//!     script="$(command "$WORKTRUNK_BIN" --internal "$@" 2>&2)" || exit_code=$?
//!     if [[ -n "$script" ]]; then
//!         eval "$script"
//!     fi
//!     return "$exit_code"
//! }
//! ```
//!
//! ## Why This Design
//!
//! This pattern (stderr for logs, stdout for script) is proven by direnv. Benefits:
//! - No FIFOs, no background processes, no job control suppression
//! - Full streaming: stderr goes directly to terminal while wt runs
//! - Simple shell wrapper: just command substitution + eval
//! - Works identically across bash, zsh, and fish
//!
//! The `--internal` flag is hidden from help output—end users never interact with it.

use std::io::{self, Write};
use std::path::{Path, PathBuf};

use super::traits::OutputHandler;

/// Directive output mode for shell integration
///
/// Buffers cd/exec directives and emits them as a shell script at the end.
///
/// See module-level documentation for protocol details.
pub struct DirectiveOutput {
    /// Cached stderr handle
    stderr: io::Stderr,
    /// Target directory for cd (set by change_directory, emitted in terminate_output)
    target_dir: Option<PathBuf>,
    /// Command to execute (set by execute, emitted in terminate_output)
    exec_command: Option<String>,
    /// Shell type for directive output formatting
    shell: crate::cli::DirectiveShell,
}

impl DirectiveOutput {
    pub fn new(shell: crate::cli::DirectiveShell) -> Self {
        Self {
            stderr: io::stderr(),
            target_dir: None,
            exec_command: None,
            shell,
        }
    }
}

impl OutputHandler for DirectiveOutput {
    fn write_message_line(&mut self, line: &str) -> io::Result<()> {
        writeln!(self.stderr, "{line}")?;
        self.stderr.flush()
    }

    fn gutter(&mut self, content: String) -> io::Result<()> {
        // Gutter content is pre-formatted with its own newlines
        write!(self.stderr, "{content}")?;
        self.stderr.flush()
    }

    fn shell_integration_hint(&mut self, _message: String) -> io::Result<()> {
        // Shell integration hints are suppressed in directive mode
        // When users run through shell wrapper, they already have integration
        Ok(())
    }

    // Note: data() uses the default which calls write_message_line() -> stderr
    // This is correct for directive mode where stdout is reserved for the final shell script

    fn change_directory(&mut self, path: &Path) -> io::Result<()> {
        // Buffer the path - will be emitted as shell script in terminate_output()
        self.target_dir = Some(path.to_path_buf());
        Ok(())
    }

    fn execute(&mut self, command: String) -> anyhow::Result<()> {
        // Buffer the command - will be emitted as shell script in terminate_output()
        self.exec_command = Some(command);
        Ok(())
    }

    fn terminate_output(&mut self) -> io::Result<()> {
        use crate::cli::DirectiveShell;

        // Reset ANSI state before returning to shell.
        // wt's colored messages set terminal state on stderr.
        write!(self.stderr, "{}", anstyle::Reset)?;
        self.stderr.flush()?;

        // Emit shell script to stdout with buffered directives
        // The shell wrapper captures this via $(...) and evals it (POSIX)
        // or | Out-String | Invoke-Expression (PowerShell)
        let mut stdout = io::stdout();

        // cd command (if target directory was set)
        if let Some(ref path) = self.target_dir {
            let path_str = path.to_string_lossy();
            match self.shell {
                DirectiveShell::Posix => {
                    // POSIX: Use single quotes, escape embedded single quotes as '\''
                    let escaped = path_str.replace('\'', "'\\''");
                    writeln!(stdout, "cd '{}'", escaped)?;
                }
                DirectiveShell::Powershell => {
                    // PowerShell: Use single quotes, escape embedded single quotes by doubling
                    let escaped = path_str.replace('\'', "''");
                    writeln!(stdout, "Set-Location '{}'", escaped)?;
                }
            }
        }

        // exec command (if one was set via --execute)
        if let Some(ref cmd) = self.exec_command {
            writeln!(stdout, "{}", cmd)?;
        }

        stdout.flush()
    }
}

impl Default for DirectiveOutput {
    fn default() -> Self {
        Self::new(crate::cli::DirectiveShell::Posix)
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    /// Test that shell script output is correctly formatted
    #[test]
    fn test_shell_script_format() {
        // Test cd command escaping
        let path = PathBuf::from("/test/path");
        let path_str = path.to_string_lossy();
        let escaped = path_str.replace('\'', "'\\''");
        let cd_cmd = format!("cd '{}'", escaped);
        assert_eq!(cd_cmd, "cd '/test/path'");
    }

    #[test]
    fn test_path_with_single_quotes() {
        // Paths with single quotes need escaping
        let path = PathBuf::from("/test/it's/path");
        let path_str = path.to_string_lossy();
        let escaped = path_str.replace('\'', "'\\''");
        let cd_cmd = format!("cd '{}'", escaped);

        // Single quote is escaped as: end quote, escaped quote, start quote
        assert_eq!(cd_cmd, "cd '/test/it'\\''s/path'");
    }

    #[test]
    fn test_path_with_spaces() {
        // Paths with spaces are handled by single quoting
        let path = PathBuf::from("/test/my path/here");
        let path_str = path.to_string_lossy();
        let escaped = path_str.replace('\'', "'\\''");
        let cd_cmd = format!("cd '{}'", escaped);
        assert_eq!(cd_cmd, "cd '/test/my path/here'");
    }

    #[test]
    fn test_path_with_special_shell_chars() {
        // Shell special chars like $, `, etc. are safe inside single quotes
        let path = PathBuf::from("/test/$HOME/`whoami`/path");
        let path_str = path.to_string_lossy();
        let escaped = path_str.replace('\'', "'\\''");
        let cd_cmd = format!("cd '{}'", escaped);

        // $ and ` are literal inside single quotes, no escaping needed
        assert_eq!(cd_cmd, "cd '/test/$HOME/`whoami`/path'");
    }

    #[test]
    fn test_powershell_path_format() {
        // PowerShell uses Set-Location and doubles single quotes for escaping
        let path = PathBuf::from("C:\\Users\\test\\path");
        let path_str = path.to_string_lossy();
        let escaped = path_str.replace('\'', "''");
        let ps_cmd = format!("Set-Location '{}'", escaped);
        assert_eq!(ps_cmd, "Set-Location 'C:\\Users\\test\\path'");
    }

    #[test]
    fn test_powershell_path_with_single_quotes() {
        // PowerShell escapes single quotes by doubling them
        let path = PathBuf::from("C:\\Users\\it's a test\\path");
        let path_str = path.to_string_lossy();
        let escaped = path_str.replace('\'', "''");
        let ps_cmd = format!("Set-Location '{}'", escaped);
        assert_eq!(ps_cmd, "Set-Location 'C:\\Users\\it''s a test\\path'");
    }

    /// Test that anstyle formatting is preserved in directive output
    #[test]
    fn test_success_preserves_anstyle() {
        use anstyle::{AnsiColor, Color, Style};

        let bold = Style::new().bold();
        let cyan = Style::new().fg_color(Some(Color::Ansi(AnsiColor::Cyan)));

        // Create a styled message
        let styled = format!("{cyan}Styled{cyan:#} {bold}message{bold:#}");

        // The styled message should contain ANSI escape codes
        assert!(
            styled.contains('\x1b'),
            "Styled message should contain ANSI escape codes"
        );

        // Directive mode preserves styling for users viewing through shell wrapper
        // Messages go to stderr which streams directly to terminal
    }

    #[test]
    fn test_color_reset_on_empty_style() {
        // BUG HYPOTHESIS from CLAUDE.md (lines 154-177):
        // Using {:#} on Style::new() produces empty string, not reset code
        use anstyle::Style;

        let empty_style = Style::new();
        let output = format!("{:#}", empty_style);

        // This is the bug: {:#} on empty style produces empty string!
        assert_eq!(
            output, "",
            "BUG: Empty style reset produces empty string, not \\x1b[0m"
        );

        // This means colors can leak: "text in color{:#}" where # is on empty Style
        // doesn't actually reset, it just removes the style prefix!
    }

    #[test]
    fn test_proper_reset_with_anstyle_reset() {
        // The correct way to reset ALL styles is anstyle::Reset
        use anstyle::Reset;

        let output = format!("{}", Reset);

        // This should produce the actual reset escape code
        assert!(
            output.contains("\x1b[0m") || output == "\x1b[0m",
            "Reset should produce actual ANSI reset code"
        );
    }

    #[test]
    fn test_nested_style_resets_leak_color() {
        // BUG HYPOTHESIS from CLAUDE.md:
        // Nested style resets can leak colors
        use anstyle::{AnsiColor, Color, Style};

        let warning = Style::new().fg_color(Some(Color::Ansi(AnsiColor::Yellow)));
        let bold = Style::new().bold();

        // BAD pattern: nested reset
        let bad_output = format!("{warning}Text with {bold}nested{bold:#} styles{warning:#}");

        // When {bold:#} resets, it might also reset the warning color!
        // We can't easily test the actual ANSI codes here, but document the issue
        println!(
            "Nested reset output: {}",
            bad_output.replace('\x1b', "\\x1b")
        );

        // GOOD pattern: compose styles
        let warning_bold = warning.bold();
        let good_output =
            format!("{warning}Text with {warning_bold}composed{warning_bold:#} styles{warning:#}");
        println!("Composed output: {}", good_output.replace('\x1b', "\\x1b"));

        // The good pattern maintains color through the bold section
    }
}
